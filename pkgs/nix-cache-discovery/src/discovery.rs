use std::collections::{BTreeSet, HashMap};

use async_trait::async_trait;
use log::trace;
use pingora::{protocols::l4::socket::SocketAddr, Error};
use pingora_load_balancing::{discovery::ServiceDiscovery, Backend, Extensions};
use url::Url;

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
    Ok(vec![BackendUrl(Url::parse("https://cache.nixos.org").unwrap())])
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