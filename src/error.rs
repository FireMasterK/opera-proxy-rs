use std::io;
use std::net::AddrParseError;
use std::time::Duration;

use hyper::http;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("{0}")]
    Message(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("http error: {0}")]
    Http(#[from] http::Error),
    #[error("hyper error: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("invalid uri: {0}")]
    InvalidUri(#[from] http::uri::InvalidUri),
    #[error("address parse error: {0}")]
    AddrParse(#[from] AddrParseError),
    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("form error: {0}")]
    Form(#[from] serde_urlencoded::ser::Error),
    #[error("request failed: {0}")]
    Wreq(#[from] wreq::Error),
    #[error("upstream proxy rejected CONNECT with status {0}")]
    UpstreamConnect(u16),
    #[error("upstream pool is empty")]
    EmptyEndpointPool,
    #[error("operation timed out after {}", humantime::format_duration(*.0))]
    Timeout(Duration),
    #[error("api error {code}: {message}")]
    Api { code: i64, message: String },
}

impl ProxyError {
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}
