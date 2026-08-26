use node2socks_cloud::{api::CloudState, open_and_migrate, router_with_state};
use rand::RngCore;
use std::{env, net::SocketAddr, path::PathBuf};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let data_dir = env::var_os("NODE2SOCKS_CLOUD_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    std::fs::create_dir_all(&data_dir)?;
    let jwt_secret = match env::var("NODE2SOCKS_CLOUD_JWT_SECRET") {
        Ok(value) if value.len() >= 32 => value.into_bytes(),
        Ok(_) => return Err("NODE2SOCKS_CLOUD_JWT_SECRET must be at least 32 characters".into()),
        Err(_) => {
            let mut generated = vec![0_u8; 48];
            rand::rng().fill_bytes(&mut generated);
            tracing::warn!("NODE2SOCKS_CLOUD_JWT_SECRET is unset; tokens expire after restart");
            generated
        }
    };
    let database = data_dir.join("node2socks-cloud.db");
    let connection = open_and_migrate(database)?;

    let address: SocketAddr = env::var("NODE2SOCKS_CLOUD_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
        .parse()?;
    let listener = TcpListener::bind(address).await?;
    tracing::info!(%address, "Node2Socks Cloud listening");
    axum::serve(
        listener,
        router_with_state(CloudState::new(connection, jwt_secret)),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
}
