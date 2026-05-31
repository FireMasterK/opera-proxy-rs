use std::sync::Arc;

use clap::Parser;
use opera_proxy_rs::config::Config;
use opera_proxy_rs::proxy::ProxyService;
use opera_proxy_rs::rotation::EndpointRotator;
use opera_proxy_rs::seclient::SEClient;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::parse();
    if let Err(err) = run(config).await {
        error!("{err}");
        std::process::exit(1);
    }
}

async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let seclient = Arc::new(SEClient::new(&config)?);
    seclient
        .initialize(&config.api_login, &config.api_password, &config.country)
        .await?;
    let endpoints = if let Some(address) = &config.override_proxy_address {
        vec![opera_proxy_rs::seclient::DiscoveredEndpoint {
            country: config.country.clone(),
            country_code: config.country.clone(),
            host: config.server_name_override.clone(),
            ip: address
                .split(':')
                .next()
                .ok_or("override proxy address must be host[:port]")?
                .to_string(),
            port: address
                .split(':')
                .nth(1)
                .and_then(|port| port.parse().ok())
                .unwrap_or(443),
        }]
    } else {
        seclient
            .refresh_endpoints(&config.api_login, &config.api_password, &config.country)
            .await?
    };

    info!("loaded {} upstream endpoints", endpoints.len());

    let rotator = Arc::new(EndpointRotator::new(config.rotation_mode, endpoints));
    spawn_refresh_loop(config.clone(), seclient.clone(), rotator.clone());

    let service = ProxyService::new(
        rotator,
        seclient,
        config.timeout,
        config.fake_sni.clone(),
    );
    service.serve(config.bind_address).await?;
    Ok(())
}

fn spawn_refresh_loop(
    config: Config,
    seclient: Arc<SEClient>,
    rotator: Arc<EndpointRotator>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.refresh);
        loop {
            interval.tick().await;

            if let Err(err) = seclient
                .refresh_login(&config.api_login, &config.api_password)
                .await
            {
                error!("login refresh failed: {err}");
                continue;
            }

            if let Err(err) = seclient
                .refresh_device_password(&config.api_login, &config.api_password)
                .await
            {
                error!("device password refresh failed: {err}");
                continue;
            }

            if config.override_proxy_address.is_none() {
                match seclient
                    .refresh_endpoints(&config.api_login, &config.api_password, &config.country)
                    .await
                {
                    Ok(endpoints) => {
                        rotator.replace_endpoints(endpoints).await;
                        info!("refreshed upstream endpoint pool");
                    }
                    Err(err) => error!("endpoint refresh failed: {err}"),
                }
            }
        }
    });
}
