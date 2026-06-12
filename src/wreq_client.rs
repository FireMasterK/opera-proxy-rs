use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use tokio::sync::RwLock;
use wreq::redirect::Policy;
use wreq_util::Emulation;

use crate::error::ProxyError;
use crate::seclient::{Credentials, DiscoveredEndpoint};

#[derive(Clone)]
pub struct UpstreamWreqClient {
    base: wreq::Client,
    fake_sni: Option<String>,
}

impl UpstreamWreqClient {
    pub fn new(timeout: Duration, fake_sni: Option<String>) -> Result<Self, ProxyError> {
        let base = wreq::Client::builder()
            .emulation(Emulation::Opera130)
            .timeout(timeout)
            .redirect(Policy::none())
            .tls_sni(false)
            .tls_verify_hostname(false)
            .build()
            .map_err(|err| ProxyError::message(err.to_string()))?;
        Ok(Self { base, fake_sni })
    }

    pub fn request_via_endpoint(
        &self,
        method: wreq::Method,
        url: &str,
        endpoint: &DiscoveredEndpoint,
        credentials: &Credentials,
    ) -> Result<wreq::RequestBuilder, ProxyError> {
        let proxy = self.proxy_for(endpoint, credentials)?;
        Ok(self.base.request(method, url).proxy(proxy))
    }

    pub async fn connect_via_endpoint(
        &self,
        target: &str,
        endpoint: &DiscoveredEndpoint,
        credentials: &Credentials,
    ) -> Result<wreq::Upgraded, ProxyError> {
        let proxy = self.proxy_for(endpoint, credentials)?;
        let connect_uri = format!("http://{target}");

        let mut builder = self
            .base
            .request(wreq::Method::CONNECT, connect_uri)
            .proxy(proxy)
            .header(
                wreq::header::PROXY_AUTHORIZATION,
                proxy_authorization(credentials)?,
            );

        if let Some(fake_sni) = &self.fake_sni {
            builder = builder.header(
                "x-forwarded-server-name",
                wreq::header::HeaderValue::from_str(fake_sni)
                    .map_err(|err| ProxyError::message(err.to_string()))?,
            );
        }

        let response = builder
            .send()
            .await
            .map_err(|err| ProxyError::message(err.to_string()))?;

        let status = response.status();
        if status != wreq::StatusCode::OK {
            return Err(ProxyError::UpstreamConnect(status.as_u16()));
        }

        response
            .upgrade()
            .await
            .map_err(|err| ProxyError::message(err.to_string()))
    }

    fn proxy_for(
        &self,
        endpoint: &DiscoveredEndpoint,
        credentials: &Credentials,
    ) -> Result<wreq::Proxy, ProxyError> {
        let scheme = if endpoint.port == 443 {
            "https"
        } else {
            "http"
        };
        let proxy_url = format!("{scheme}://{}", endpoint.addr());
        let mut proxy =
            wreq::Proxy::all(proxy_url).map_err(|err| ProxyError::message(err.to_string()))?;
        proxy = proxy.basic_auth(&credentials.login, &credentials.password);

        if let Some(fake_sni) = &self.fake_sni {
            let mut headers = wreq::header::HeaderMap::new();
            headers.insert(
                "x-forwarded-server-name",
                wreq::header::HeaderValue::from_str(fake_sni)
                    .map_err(|err| ProxyError::message(err.to_string()))?,
            );
            proxy = proxy.custom_http_headers(headers);
        }

        Ok(proxy)
    }
}

fn proxy_authorization(credentials: &Credentials) -> Result<wreq::header::HeaderValue, ProxyError> {
    let token = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", credentials.login, credentials.password));
    wreq::header::HeaderValue::from_str(&format!("Basic {token}"))
        .map_err(|err| ProxyError::message(err.to_string()))
}

#[derive(Clone, Default)]
pub struct ClientCache {
    inner: Arc<RwLock<Option<UpstreamWreqClient>>>,
}

impl ClientCache {
    pub async fn get_or_init(
        &self,
        timeout: Duration,
        fake_sni: Option<String>,
    ) -> Result<UpstreamWreqClient, ProxyError> {
        if let Some(client) = self.inner.read().await.clone() {
            return Ok(client);
        }

        let client = UpstreamWreqClient::new(timeout, fake_sni)?;
        *self.inner.write().await = Some(client.clone());
        Ok(client)
    }
}
