use std::sync::Arc;

use base64::Engine as _;
use digest_auth::{AuthContext, HttpMethod, WwwAuthenticateHeader};
use reqwest::cookie::Jar;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT, WWW_AUTHENTICATE,
};
use reqwest::{Client, RequestBuilder};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::Config;
use crate::error::ProxyError;

const ANON_EMAIL_LOCALPART_BYTES: usize = 32;
const DEVICE_ID_BYTES: usize = 20;
const SE_STATUS_OK: i64 = 0;

#[derive(Debug, Clone)]
pub struct SEEndpoints {
    pub register_subscriber: String,
    pub subscriber_login: String,
    pub register_device: String,
    pub device_generate_password: String,
    pub geo_list: String,
    pub discover: String,
}

impl Default for SEEndpoints {
    fn default() -> Self {
        Self {
            register_subscriber: "https://api2.sec-tunnel.com/v4/register_subscriber".into(),
            subscriber_login: "https://api2.sec-tunnel.com/v4/subscriber_login".into(),
            register_device: "https://api2.sec-tunnel.com/v4/register_device".into(),
            device_generate_password: "https://api2.sec-tunnel.com/v4/device_generate_password".into(),
            geo_list: "https://api2.sec-tunnel.com/v4/geo_list".into(),
            discover: "https://api2.sec-tunnel.com/v4/discover".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SESettings {
    pub client_version: String,
    pub client_type: String,
    pub device_name: String,
    pub operating_system: String,
    pub user_agent: String,
    pub endpoints: SEEndpoints,
}

impl Default for SESettings {
    fn default() -> Self {
        Self {
            client_version: "Stable 114.0.5282.21".into(),
            client_type: "se0316".into(),
            device_name: "Opera-Browser-Client".into(),
            operating_system: "Windows".into(),
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36 OPR/114.0.0.0".into(),
            endpoints: SEEndpoints::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub login: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct DiscoveredEndpoint {
    pub country: String,
    pub country_code: String,
    pub host: Option<String>,
    pub ip: String,
    pub port: u16,
}

impl DiscoveredEndpoint {
    pub fn addr(&self) -> String {
        format!("{}:{}", self.ip, self.port)
    }

    pub fn tls_server_name(&self) -> String {
        self.host
            .clone()
            .unwrap_or_else(|| format!("{}0.sec-tunnel.com", self.country_code.to_lowercase()))
    }
}

#[derive(Debug)]
struct State {
    subscriber_email: String,
    subscriber_password: String,
    device_id: String,
    assigned_device_id: String,
    assigned_device_id_hash: String,
    device_password: String,
}

impl State {
    fn new(device_id: String) -> Self {
        Self {
            subscriber_email: String::new(),
            subscriber_password: String::new(),
            device_id,
            assigned_device_id: String::new(),
            assigned_device_id_hash: String::new(),
            device_password: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SEClient {
    http_client: Client,
    settings: SESettings,
    state: Arc<Mutex<State>>,
}

impl SEClient {
    pub fn new(config: &Config) -> Result<Self, ProxyError> {
        let settings = SESettings::default();
        let device_id = random_capital_hex_string(DEVICE_ID_BYTES)?;
        let jar = Arc::new(Jar::default());

        let http_client = Client::builder()
            .cookie_provider(jar)
            .timeout(config.timeout)
            .build()?;

        Ok(Self {
            http_client,
            settings,
            state: Arc::new(Mutex::new(State::new(device_id))),
        })
    }

    pub async fn initialize(&self, api_login: &str, api_password: &str, country: &str) -> Result<Vec<DiscoveredEndpoint>, ProxyError> {
        self.anon_register(api_login, api_password).await?;
        self.register_device(api_login, api_password).await?;
        self.discover(api_login, api_password, country).await
    }

    pub async fn refresh_endpoints(
        &self,
        api_login: &str,
        api_password: &str,
        country: &str,
    ) -> Result<Vec<DiscoveredEndpoint>, ProxyError> {
        self.discover(api_login, api_password, country).await
    }

    pub async fn refresh_login(&self, api_login: &str, api_password: &str) -> Result<(), ProxyError> {
        let mut state = self.state.lock().await;
        self.login_locked(&mut state, api_login, api_password).await
    }

    pub async fn refresh_device_password(&self, api_login: &str, api_password: &str) -> Result<(), ProxyError> {
        let mut state = self.state.lock().await;
        self.device_generate_password_locked(&mut state, api_login, api_password).await
    }

    pub async fn get_proxy_credentials(&self) -> Credentials {
        let state = self.state.lock().await;
        Credentials {
            login: state.assigned_device_id_hash.clone(),
            password: state.device_password.clone(),
        }
    }

    async fn anon_register(&self, api_login: &str, api_password: &str) -> Result<(), ProxyError> {
        let mut state = self.state.lock().await;
        let local_part = random_email_local_part(ANON_EMAIL_LOCALPART_BYTES)?;
        state.subscriber_email = format!("{local_part}@{}.best.vpn", self.settings.client_type);
        state.subscriber_password = capital_hex_sha1(&state.subscriber_email);
        self.register_locked(&mut state, api_login, api_password).await
    }

    async fn register_locked(&self, state: &mut State, api_login: &str, api_password: &str) -> Result<(), ProxyError> {
        let response: RegisterSubscriberResponse = self
            .rpc_call(
                api_login,
                api_password,
                &self.settings.endpoints.register_subscriber,
                &[
                    ("email", state.subscriber_email.as_str()),
                    ("password", state.subscriber_password.as_str()),
                ],
            )
            .await?;
        ensure_ok(response.status)
    }

    async fn register_device(&self, api_login: &str, api_password: &str) -> Result<(), ProxyError> {
        let mut state = self.state.lock().await;
        let response: RegisterDeviceResponse = self
            .rpc_call(
                api_login,
                api_password,
                &self.settings.endpoints.register_device,
                &[
                    ("client_type", self.settings.client_type.as_str()),
                    ("device_hash", state.device_id.as_str()),
                    ("device_name", self.settings.device_name.as_str()),
                ],
            )
            .await?;
        ensure_ok(response.status)?;
        state.assigned_device_id = response.data.device_id.clone();
        state.assigned_device_id_hash = capital_hex_sha1(&response.data.device_id);
        state.device_password = response.data.device_password;
        Ok(())
    }

    async fn discover(
        &self,
        api_login: &str,
        api_password: &str,
        country: &str,
    ) -> Result<Vec<DiscoveredEndpoint>, ProxyError> {
        let state = self.state.lock().await;
        let response: DiscoverResponse = self
            .rpc_call(
                api_login,
                api_password,
                &self.settings.endpoints.discover,
                &[
                    ("serial_no", state.assigned_device_id_hash.as_str()),
                    ("requested_geo", country),
                ],
            )
            .await?;
        ensure_ok(response.status)?;
        let endpoints = response
            .data
            .ips
            .into_iter()
            .map(|entry| DiscoveredEndpoint {
                country: entry.geo.country.unwrap_or_default(),
                country_code: entry.geo.country_code,
                host: entry.host,
                ip: entry.ip,
                port: entry.ports.into_iter().next().unwrap_or(443),
            })
            .collect();
        Ok(endpoints)
    }

    async fn login_locked(
        &self,
        state: &mut State,
        api_login: &str,
        api_password: &str,
    ) -> Result<(), ProxyError> {
        let response: SubscriberLoginResponse = self
            .rpc_call(
                api_login,
                api_password,
                &self.settings.endpoints.subscriber_login,
                &[
                    ("login", state.subscriber_email.as_str()),
                    ("password", state.subscriber_password.as_str()),
                    ("client_type", self.settings.client_type.as_str()),
                ],
            )
            .await?;
        ensure_ok(response.status)
    }

    async fn device_generate_password_locked(
        &self,
        state: &mut State,
        api_login: &str,
        api_password: &str,
    ) -> Result<(), ProxyError> {
        let response: DeviceGeneratePasswordResponse = self
            .rpc_call(
                api_login,
                api_password,
                &self.settings.endpoints.device_generate_password,
                &[("device_id", state.assigned_device_id.as_str())],
            )
            .await?;
        ensure_ok(response.status)?;
        state.device_password = response.data.device_password;
        Ok(())
    }

    async fn rpc_call<T: for<'de> Deserialize<'de>>(
        &self,
        api_login: &str,
        api_password: &str,
        endpoint: &str,
        params: &[(&str, &str)],
    ) -> Result<T, ProxyError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "SE-Client-Version",
            HeaderValue::from_str(&self.settings.client_version)
                .map_err(|err| ProxyError::message(err.to_string()))?,
        );
        headers.insert(
            "SE-Operating-System",
            HeaderValue::from_str(&self.settings.operating_system)
                .map_err(|err| ProxyError::message(err.to_string()))?,
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&self.settings.user_agent)
                .map_err(|err| ProxyError::message(err.to_string()))?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        let body = serde_urlencoded::to_string(params)?;
        let request = self
            .http_client
            .post(endpoint)
            .headers(headers)
            .body(body);

        let response = self
            .send_digest_authed(request, api_login, api_password, endpoint)
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProxyError::message(format!(
                "bad http status from SurfEasy API: {status}"
            )));
        }
        Ok(response.json::<T>().await?)
    }

    async fn send(&self, request: RequestBuilder) -> Result<reqwest::Response, ProxyError> {
        let response = request.send().await?;
        debug!("SurfEasy response status={}", response.status());
        Ok(response)
    }

    async fn send_digest_authed(
        &self,
        request: RequestBuilder,
        username: &str,
        password: &str,
        endpoint: &str,
    ) -> Result<reqwest::Response, ProxyError> {
        let first = request
            .try_clone()
            .ok_or_else(|| ProxyError::message("unable to clone request for digest auth"))?
            .send()
            .await?;

        if first.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(first);
        }

        let challenge = first
            .headers()
            .get(WWW_AUTHENTICATE)
            .ok_or_else(|| ProxyError::message("missing WWW-Authenticate challenge"))?
            .to_str()
            .map_err(|err| ProxyError::message(err.to_string()))?;
        let mut prompt = WwwAuthenticateHeader::parse(challenge)
            .map_err(|err| ProxyError::message(err.to_string()))?;
        let endpoint_url = url::Url::parse(endpoint)?;
        let auth = AuthContext::new_with_method(
            username,
            password,
            endpoint_url.path(),
            Option::<&[u8]>::None,
            HttpMethod::POST,
        );
        let authorization = prompt
            .respond(&auth)
            .map_err(|err| ProxyError::message(err.to_string()))?
            .to_string();

        self.send(request.header(AUTHORIZATION, authorization)).await
    }
}

fn ensure_ok(status: SEStatusInner) -> Result<(), ProxyError> {
    if status.code == SE_STATUS_OK {
        Ok(())
    } else {
        Err(ProxyError::Api {
            code: status.code,
            message: status.message,
        })
    }
}

fn random_email_local_part(bytes: usize) -> Result<String, ProxyError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|err| ProxyError::message(err.to_string()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf))
}

fn random_capital_hex_string(bytes: usize) -> Result<String, ProxyError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|err| ProxyError::message(err.to_string()))?;
    Ok(hex_upper(&buf))
}

fn capital_hex_sha1(input: &str) -> String {
    let digest = Sha1::digest(input.as_bytes());
    hex_upper(digest.as_slice())
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

#[derive(Debug)]
struct SEStatusInner {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct RegisterSubscriberResponse {
    #[serde(flatten)]
    status: SEStatusInner,
}

#[derive(Debug, Deserialize)]
struct RegisterDeviceResponse {
    data: RegisterDeviceData,
    #[serde(flatten)]
    status: SEStatusInner,
}

#[derive(Debug, Deserialize)]
struct RegisterDeviceData {
    device_id: String,
    device_password: String,
}

#[derive(Debug, Deserialize)]
struct DeviceGeneratePasswordResponse {
    data: DeviceGeneratePasswordData,
    #[serde(flatten)]
    status: SEStatusInner,
}

#[derive(Debug, Deserialize)]
struct DeviceGeneratePasswordData {
    device_password: String,
}

#[derive(Debug, Deserialize)]
struct SubscriberLoginResponse {
    #[serde(flatten)]
    status: SEStatusInner,
}

#[derive(Debug, Deserialize)]
struct DiscoverResponse {
    data: DiscoverData,
    #[serde(flatten)]
    status: SEStatusInner,
}

#[derive(Debug, Deserialize)]
struct DiscoverData {
    ips: Vec<DiscoverIpEntry>,
}

#[derive(Debug, Deserialize)]
struct DiscoverIpEntry {
    geo: DiscoverGeoEntry,
    host: Option<String>,
    ip: String,
    ports: Vec<u16>,
}

#[derive(Debug, Deserialize)]
struct DiscoverGeoEntry {
    country: Option<String>,
    country_code: String,
}

fn deserialize_status_pair<'de, D>(deserializer: D) -> Result<SEStatusInner, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let object = value
        .get("return_code")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| serde::de::Error::custom("return_code must be an object"))?;
    if object.len() != 1 {
        return Err(serde::de::Error::custom("ambiguous status object"));
    }
    let (code, message) = object
        .iter()
        .next()
        .ok_or_else(|| serde::de::Error::custom("missing status pair"))?;
    let parsed_code = code
        .parse::<i64>()
        .map_err(|err| serde::de::Error::custom(err.to_string()))?;
    let parsed_message = message
        .as_str()
        .ok_or_else(|| serde::de::Error::custom("status message must be a string"))?;
    Ok(SEStatusInner {
        code: parsed_code,
        message: parsed_message.to_string(),
    })
}

impl<'de> Deserialize<'de> for SEStatusInner {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_status_pair(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_pair() {
        let raw = r#"{"return_code":{"0":"OK"}}"#;
        let decoded: RegisterSubscriberResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(decoded.status.code, 0);
        assert_eq!(decoded.status.message, "OK");
    }

    #[test]
    fn hashes_match_go_reference() {
        assert_eq!(
            capital_hex_sha1("test@example.com"),
            "567159D622FFBB50B11B0EFD307BE358624A26EE"
        );
    }
}
