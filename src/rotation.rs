use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rand::RngExt as _;
use tokio::sync::RwLock;

use crate::config::RotationMode;
use crate::error::ProxyError;
use crate::seclient::DiscoveredEndpoint;

#[derive(Debug)]
pub struct EndpointRotator {
    mode: RotationMode,
    next_index: AtomicUsize,
    endpoints: Arc<RwLock<Vec<DiscoveredEndpoint>>>,
}

impl EndpointRotator {
    pub fn new(mode: RotationMode, endpoints: Vec<DiscoveredEndpoint>) -> Self {
        Self {
            mode,
            next_index: AtomicUsize::new(0),
            endpoints: Arc::new(RwLock::new(endpoints)),
        }
    }

    pub async fn replace_endpoints(&self, endpoints: Vec<DiscoveredEndpoint>) {
        let mut guard = self.endpoints.write().await;
        *guard = endpoints;
        self.next_index.store(0, Ordering::Relaxed);
    }

    pub async fn choose(&self) -> Result<DiscoveredEndpoint, ProxyError> {
        let guard = self.endpoints.read().await;
        if guard.is_empty() {
            return Err(ProxyError::EmptyEndpointPool);
        }

        let index = match self.mode {
            RotationMode::RoundRobin => {
                let current = self.next_index.fetch_add(1, Ordering::Relaxed);
                current % guard.len()
            }
            RotationMode::Random => rand::rng().random_range(0..guard.len()),
        };

        Ok(guard[index].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(ip: &str) -> DiscoveredEndpoint {
        DiscoveredEndpoint {
            country: "Europe".into(),
            country_code: "EU".into(),
            host: None,
            ip: ip.into(),
            port: 443,
        }
    }

    #[tokio::test]
    async fn round_robin_rotates() {
        let rotator = EndpointRotator::new(
            RotationMode::RoundRobin,
            vec![endpoint("1.1.1.1"), endpoint("2.2.2.2")],
        );

        let first = rotator.choose().await.unwrap();
        let second = rotator.choose().await.unwrap();
        let third = rotator.choose().await.unwrap();

        assert_eq!(first.ip, "1.1.1.1");
        assert_eq!(second.ip, "2.2.2.2");
        assert_eq!(third.ip, "1.1.1.1");
    }
}
