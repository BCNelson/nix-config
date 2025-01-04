use async_trait::async_trait;
use log::trace;
use pingora::Error;
use pingora_load_balancing::{health_check::HealthCheck, Backend};



pub struct Healthcheck ();

#[async_trait]
impl HealthCheck for Healthcheck {
    async fn check(&self, _backend: &Backend) -> Result<(), Box<Error>> {
        trace!("Healthcheck::check");
        // Make a request to the 
        Ok(())
    }

    fn health_threshold(&self, _sucess: bool) -> usize {
        1
    }
}