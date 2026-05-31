use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "opera-proxy-rs")]
#[command(about = "Rust HTTP forward proxy for Opera/SurfEasy endpoints")]
pub struct Config {
    #[arg(long, default_value = "127.0.0.1:18080")]
    pub bind_address: SocketAddr,
    #[arg(long, default_value = "EU")]
    pub country: String,
    #[arg(long, default_value = "4h", value_parser = parse_duration)]
    pub refresh: Duration,
    #[arg(long, default_value = "30s", value_parser = parse_duration)]
    pub timeout: Duration,
    #[arg(long, default_value = "se0316")]
    pub api_login: String,
    #[arg(long, default_value = "SILrMEPBmJuhomxWkfm3JalqHX2Eheg1YhlEZiMh8II")]
    pub api_password: String,
    #[arg(long)]
    pub fake_sni: Option<String>,
    #[arg(long)]
    pub override_proxy_address: Option<String>,
    #[arg(long)]
    pub server_name_override: Option<String>,
    #[arg(long, default_value_t = RotationMode::RoundRobin)]
    pub rotation_mode: RotationMode,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, Default)]
pub enum RotationMode {
    #[default]
    RoundRobin,
    Random,
}

impl std::fmt::Display for RotationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::RoundRobin => "round-robin",
            Self::Random => "random",
        };
        f.write_str(value)
    }
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    humantime::parse_duration(value).map_err(|err| err.to_string())
}
