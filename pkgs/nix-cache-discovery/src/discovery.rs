use std::{collections::{BTreeSet, HashMap}, time::Duration};

use async_trait::async_trait;
use log::{info, trace};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use pingora::{protocols::l4::socket::SocketAddr, Error};
use pingora_load_balancing::{discovery::ServiceDiscovery, Backend, Extensions};
use url::Url;

use tokio::time::interval;

pub struct BackendUrl(Url);



pub struct MdnsServiceDiscovery ();

impl MdnsServiceDiscovery {
    pub fn new() -> Self {
        MdnsServiceDiscovery()
    }
}

fn url_to_backends(url: Url) -> Vec<Backend> {
    let mut ret: Vec<Backend> = Vec::new();
    for addr in url.socket_addrs(|| None).unwrap() {
        let mut ext = Extensions::new();
        ext.insert(url.clone());
        ret.push(Backend {
            addr: SocketAddr::Inet(addr),
            weight: 1,
            ext,
        });
    }
    ret
}

async fn get_backend_urls() -> Result<Vec<BackendUrl>, Box<Error>> {
    trace!("get_backend_urls");
    let mut ret = vec![];
    ret.push(BackendUrl(Url::parse("https://cache.nixos.org").unwrap()));
    let mut period = interval(Duration::from_secs(2));
    period.tick().await;

    let service_type = "_nix-binary-cache._sub._https._tcp.local.";
    let mdns = ServiceDaemon::new().expect("Failed to create daemon");

    info!("Browsing for service: {:?}", service_type);
    let receiver = mdns.browse(service_type).expect("Failed to browse");
    loop {
        tokio::select! {
            Ok(event) = receiver.recv_async() => {
                match event {
                    ServiceEvent::SearchStarted(_) => {
                        info!("Service started");
                    }
                    ServiceEvent::SearchStopped(_) => {
                        info!("Service stopped");
                        break;
                    }
                    ServiceEvent::ServiceFound(service_type, fullname) => {
                        info!("Service found service_type:{:?} fullname:{:?}", service_type, fullname);
                    }
                    ServiceEvent::ServiceResolved(service) => {
                        info!("Service resolved: {:?}", service);
                        match service.get_property_val_str("url") {
                            Some(url) => {
                                info!("Service resolved url: {:?}", url);
                                match Url::parse(&url) {
                                    Ok(url) => {
                                        ret.push(BackendUrl(url));
                                    }
                                    Err(e) => {
                                        info!("Service resolved url parse error: {:?}", e);
                                        continue;
                                    }
                                }
                            }
                            None => {
                                info!("Service resolved url not found");
                            }
                        }
                    }
                    ServiceEvent::ServiceRemoved(service_type, fullname) => {
                        info!("Service found service_type:{:?} fullname:{:?}", service_type, fullname);
                    }
                }
            }
            _ = period.tick() => {
                info!("timeout");
                break;
            }
        }
    }

    info!("Shutting down mdns daemon");
    mdns.shutdown().unwrap();

    Ok(ret)
}


#[async_trait]
impl ServiceDiscovery for MdnsServiceDiscovery {
    async fn discover(&self) -> Result<(BTreeSet<Backend>, HashMap<u64, bool>), Box<Error>> {
        trace!("discovering backends");

        let mut backends = BTreeSet::new();

        for backend_url in get_backend_urls().await? {
            trace!("discovered backend url: {:?}", backend_url.0);
            for backend in url_to_backends(backend_url.0){
                trace!("discovered backend: {:?}", backend);
                backends.insert(backend);
            }
        }

        trace!("discovered backends: {:?}", backends);
        
        Ok((backends, HashMap::new()))
    }
}