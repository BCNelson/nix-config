use async_trait::async_trait;
use discovery::MdnsServiceDiscovery;
use log::{info, LevelFilter};
use pingora::prelude::*;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_core::Result;
use pingora_load_balancing::{discovery::ServiceDiscovery, Backends};
use pingora_proxy::{ProxyHttp, Session};
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::sync::Arc;
use url::Url;

mod discovery;
mod healthcheck;

pub struct LB(Arc<LoadBalancer<RoundRobin>>);

impl LB {
    pub fn new(discovery: Box<dyn ServiceDiscovery + Send + Sync>) -> Self {
        let mut backends = Backends::new(discovery);
        backends.set_health_check(Box::new(healthcheck::Healthcheck()));
        let mut lb = LoadBalancer::from_backends(backends);
        lb.health_check_frequency = Some(std::time::Duration::from_secs(10));
        lb.update_frequency = Some(std::time::Duration::from_secs(3));
        LB(Arc::new(lb))
    }
}

#[async_trait]
impl ProxyHttp for LB {
    type CTX = (Option<String>, bool); // (host, tls)
    fn new_ctx(&self) -> Self::CTX {
        (None, false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let upstream = match self.0.select(b"", 256) {
            Some(upstream) => upstream,
            None => return Err(pingora::Error::new_str("No upstream found")),
        };

        println!("upstream peer is: {upstream:?}");

        let (tls, sni) = match upstream.ext.get::<Url>() {
            Some(url) => (
                url.scheme() == "https",
                match url.host_str() {
                    Some(host) => Some(host.to_string()),
                    None => None,
                },
            ),
            None => (false, None),
        };

        ctx.0 = sni.clone();
        ctx.1 = tls;

        let peer = Box::new(HttpPeer::new(upstream, tls, sni.unwrap_or("".to_string())));
        Ok(peer)
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        match ctx.0 {
            Some(ref host) => upstream_request.insert_header("Host", host),
            None => Ok(()),
        }
    }
}

fn main() {
    CombinedLogger::init(
        vec![
            TermLogger::new(LevelFilter::Trace, Config::default(), TerminalMode::Mixed, ColorChoice::Auto)
        ]
    ).unwrap();
    info!("Starting service");
    let mut my_server = Server::new(None).unwrap();
    let mdns_service_discovery = MdnsServiceDiscovery::new();
    let mut backends = Backends::new(Box::new(mdns_service_discovery));
    backends.set_health_check(Box::new(healthcheck::Healthcheck()));
    let mut lb = LoadBalancer::from_backends(backends);
    lb.health_check_frequency = Some(std::time::Duration::from_secs(10));
    lb.update_frequency = Some(std::time::Duration::from_secs(30));

    let backgroud = background_service("LB-BD", lb);



    let mut proxy_service = http_proxy_service(&my_server.configuration, LB(backgroud.task()));
    proxy_service.add_tcp("0.0.0.0:2151");

    my_server.add_service(backgroud);
    my_server.add_service(proxy_service);
    my_server.bootstrap();
    info!("Server started");
    my_server.run_forever();
}