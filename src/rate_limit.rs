use axum::extract::ConnectInfo;
use axum::http::request::Request;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tower_governor::errors::GovernorError;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;

/// Unauthenticated bearer strings never determine a rate-limit bucket.
/// TODO(air): add verified-agent limits if shared-IP traffic needs more capacity;
/// keep this IP guard before authentication to bound hashing and mail abuse.
#[derive(Debug, Clone)]
pub struct IpKeyExtractor {
    pub trusted_proxies: Vec<IpAddr>,
}

impl KeyExtractor for IpKeyExtractor {
    type Key = IpAddr;
    fn extract<T>(&self, req: &Request<T>) -> Result<IpAddr, GovernorError> {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        if let Some(peer) = peer {
            if !self.trusted_proxies.contains(&peer) {
                return Ok(peer);
            }
        }
        // No ConnectInfo occurs only in router tests; main always installs it.
        // Caddy overwrites X-Forwarded-For from untrusted clients.
        req.headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
            .or(peer)
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

fn config(
    seconds: u64,
    burst: u32,
) -> Arc<GovernorConfig<IpKeyExtractor, governor::middleware::NoOpMiddleware>> {
    let trusted_proxies = std::env::var("TRUSTED_PROXY_IPS")
        .unwrap_or_default()
        .split(',')
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            v.trim()
                .parse()
                .expect("TRUSTED_PROXY_IPS must contain IP addresses")
        })
        .collect();
    Arc::new(
        GovernorConfigBuilder::default()
            .per_second(seconds)
            .burst_size(burst)
            .key_extractor(IpKeyExtractor { trusted_proxies })
            .finish()
            .expect("valid rate limiter"),
    )
}

/// One request per second, with an initial burst of one hundred per client IP.
/// Allows multiple background agents operating from a shared IP/VPS to coordinate without false-positive 429s.
pub fn build() -> Arc<GovernorConfig<IpKeyExtractor, governor::middleware::NoOpMiddleware>> {
    config(1, 100)
}

/// Three initial sign-in requests, replenishing one every minute.
pub fn signin() -> Arc<GovernorConfig<IpKeyExtractor, governor::middleware::NoOpMiddleware>> {
    config(60, 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarded_ip_is_only_trusted_from_configured_proxy() {
        let proxy: IpAddr = "172.18.0.5".parse().expect("proxy");
        let client: IpAddr = "203.0.113.10".parse().expect("client");
        let extractor = IpKeyExtractor {
            trusted_proxies: vec![proxy],
        };
        let mut req = Request::builder()
            .header("x-forwarded-for", "203.0.113.10")
            .body(())
            .expect("request");
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(proxy, 1000)));
        assert_eq!(extractor.extract(&req).expect("key"), client);
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(client, 1000)));
        req.headers_mut()
            .insert("x-forwarded-for", "192.0.2.77".parse().expect("header"));
        assert_eq!(extractor.extract(&req).expect("key"), client);
    }
}
