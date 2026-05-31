use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{
    CONNECTION, HOST, HeaderMap, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER,
    TRANSFER_ENCODING, UPGRADE,
};
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tracing::{error, info};

use crate::error::ProxyError;
use crate::relay::tunnel_bidirectional;
use crate::rotation::EndpointRotator;
use crate::seclient::SEClient;
use crate::wreq_client::ClientCache;

type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

#[derive(Clone)]
pub struct ProxyService {
    rotator: Arc<EndpointRotator>,
    seclient: Arc<SEClient>,
    client_cache: ClientCache,
    timeout: Duration,
    fake_sni: Option<String>,
}

impl ProxyService {
    pub fn new(
        rotator: Arc<EndpointRotator>,
        seclient: Arc<SEClient>,
        timeout: Duration,
        fake_sni: Option<String>,
    ) -> Self {
        Self {
            rotator,
            seclient,
            client_cache: ClientCache::default(),
            timeout,
            fake_sni,
        }
    }

    pub async fn serve(self, bind_address: std::net::SocketAddr) -> Result<(), ProxyError> {
        let listener = TcpListener::bind(bind_address).await?;
        let mut tasks = JoinSet::new();

        loop {
            let (stream, remote_addr) = listener.accept().await?;
            let service = self.clone();
            tasks.spawn(async move {
                let io = TokioIo::new(stream);
                let result = http1::Builder::new()
                    .preserve_header_case(true)
                    .title_case_headers(true)
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let inner = service.clone();
                            async move { inner.handle(req).await }
                        }),
                    )
                    .with_upgrades()
                    .await;
                if let Err(err) = result {
                    error!("connection from {remote_addr} failed: {err}");
                }
            });
        }
    }

    async fn handle(&self, req: Request<Incoming>) -> Result<Response<BoxBody>, Infallible> {
        let result = if req.method() == Method::CONNECT {
            self.handle_connect(req).await
        } else {
            self.handle_http(req).await
        };

        let response = match result {
            Ok(response) => response,
            Err(err) => {
                error!("proxy error: {err}");
                simple_response(StatusCode::BAD_GATEWAY, err.to_string())
            }
        };

        Ok(response)
    }

    async fn handle_connect(&self, req: Request<Incoming>) -> Result<Response<BoxBody>, ProxyError> {
        let authority = req
            .uri()
            .authority()
            .map(|authority| authority.as_str().to_string())
            .or_else(|| req.uri().path().strip_prefix('/').map(str::to_owned))
            .ok_or_else(|| ProxyError::message("CONNECT request is missing authority"))?;

        let on_upgrade = upgrade::on(req);
        let endpoint = self.rotator.choose().await?;
        let credentials = self.seclient.get_proxy_credentials().await;
        let client = self
            .client_cache
            .get_or_init(self.timeout, self.fake_sni.clone())
            .await?;

        tokio::spawn(async move {
            match on_upgrade.await {
                Ok(upgraded) => {
                    let mut downstream = TokioIo::new(upgraded);
                    match client
                        .connect_via_endpoint(&authority, &endpoint, &credentials)
                        .await
                    {
                        Ok(mut upstream) => {
                            if let Err(err) =
                                tunnel_bidirectional(&mut downstream, &mut upstream).await
                            {
                                error!("tunnel relay failed: {err}");
                            }
                        }
                        Err(err) => error!("upstream CONNECT failed: {err}"),
                    }
                }
                Err(err) => error!("upgrade failed: {err}"),
            }
        });

        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(empty_body())
            .expect("response builder"))
    }

    async fn handle_http(&self, req: Request<Incoming>) -> Result<Response<BoxBody>, ProxyError> {
        let method = req.method().clone();
        let headers = req.headers().clone();
        let target = absolute_target(req.uri(), &headers)?;
        let endpoint = self.rotator.choose().await?;
        let credentials = self.seclient.get_proxy_credentials().await;
        let client = self
            .client_cache
            .get_or_init(self.timeout, self.fake_sni.clone())
            .await?;

        let body = req.into_body().collect().await?.to_bytes();
        let mut builder = client.request_via_endpoint(
            method_to_wreq(method),
            &target.uri.to_string(),
            &endpoint,
            &credentials,
        )?;
        builder = builder.headers(http_to_wreq_headers(&headers)?);
        if !body.is_empty() {
            builder = builder.body(body);
        }

        let upstream_response = builder
            .send()
            .await
            .map_err(|err| ProxyError::message(err.to_string()))?;
        let status = upstream_response.status();
        let response_headers = wreq_to_http_headers(upstream_response.headers())?;
        let bytes = upstream_response
            .bytes()
            .await
            .map_err(|err| ProxyError::message(err.to_string()))?;

        let mut response = Response::builder().status(status);
        for (name, value) in &response_headers {
            response = response.header(name, value);
        }
        let mut response = response
            .body(Full::new(bytes).map_err(|never| match never {}).boxed())
            .expect("response builder");
        strip_hop_headers(response.headers_mut());
        info!("proxied HTTP request to {}", target.authority);
        Ok(response)
    }
}

struct AbsoluteTarget {
    uri: Uri,
    authority: String,
}

fn absolute_target(uri: &Uri, headers: &HeaderMap) -> Result<AbsoluteTarget, ProxyError> {
    if uri.scheme().is_some() && uri.authority().is_some() {
        let authority = uri
            .authority()
            .map(|value| value.as_str().to_owned())
            .ok_or_else(|| ProxyError::message("missing request authority"))?;
        let normalized = Uri::builder()
            .scheme(uri.scheme_str().unwrap_or("http"))
            .authority(authority.as_str())
            .path_and_query(uri.path_and_query().map(|v| v.as_str()).unwrap_or("/"))
            .build()?;
        return Ok(AbsoluteTarget {
            uri: normalized,
            authority,
        });
    }

    let host = headers
        .get(HOST)
        .ok_or_else(|| ProxyError::message("missing Host header"))?
        .to_str()
        .map_err(|err| ProxyError::message(err.to_string()))?;
    let normalized = Uri::builder()
        .scheme("http")
        .authority(host)
        .path_and_query(uri.path_and_query().map(|v| v.as_str()).unwrap_or("/"))
        .build()?;
    Ok(AbsoluteTarget {
        uri: normalized,
        authority: host.to_string(),
    })
}

pub fn strip_hop_headers(headers: &mut HeaderMap) {
    for name in [
        CONNECTION,
        http::header::HeaderName::from_static("keep-alive"),
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        http::header::HeaderName::from_static("proxy-connection"),
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(name);
    }
}

fn simple_response(status: StatusCode, message: impl Into<String>) -> Response<BoxBody> {
    Response::builder()
        .status(status)
        .body(
            Full::new(Bytes::from(message.into()))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("response builder")
}

fn empty_body() -> BoxBody {
    Empty::<Bytes>::new().map_err(|never| match never {}).boxed()
}

fn http_to_wreq_headers(headers: &HeaderMap) -> Result<wreq::header::HeaderMap, ProxyError> {
    let mut out = wreq::header::HeaderMap::new();
    for (name, value) in headers {
        out.insert(
            wreq::header::HeaderName::from_bytes(name.as_str().as_bytes())
                .map_err(|err| ProxyError::message(err.to_string()))?,
            wreq::header::HeaderValue::from_bytes(value.as_bytes())
                .map_err(|err| ProxyError::message(err.to_string()))?,
        );
    }
    Ok(out)
}

fn wreq_to_http_headers(headers: &wreq::header::HeaderMap) -> Result<HeaderMap, ProxyError> {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        out.append(
            http::header::HeaderName::from_bytes(name.as_str().as_bytes())
                .map_err(|err| ProxyError::message(err.to_string()))?,
            http::header::HeaderValue::from_bytes(value.as_bytes())
                .map_err(|err| ProxyError::message(err.to_string()))?,
        );
    }
    Ok(out)
}

fn method_to_wreq(method: Method) -> wreq::Method {
    wreq::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(wreq::Method::GET)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_hop_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, "keep-alive".parse().unwrap());
        headers.insert(PROXY_AUTHORIZATION, "Basic abc".parse().unwrap());
        headers.insert(HOST, "example.com".parse().unwrap());

        strip_hop_headers(&mut headers);

        assert!(headers.get(CONNECTION).is_none());
        assert!(headers.get(PROXY_AUTHORIZATION).is_none());
        assert_eq!(headers.get(HOST).unwrap(), "example.com");
    }
}
