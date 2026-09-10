//! `RemoteX` M8 PostgreSQL-backed control-server composition root.

use anyhow::Context;
use remotex_control::{ApiState, ControlConfig, ControlService, PostgresRepository, router};
use sqlx::postgres::PgPoolOptions;
use std::{net::SocketAddr, sync::Arc};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?;

    let database_url = required("REMOTEX_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(parse_or("REMOTEX_CONTROL_DB_CONNECTIONS", 10_u32)?)
        .connect(&database_url)
        .await
        .context("connect to PostgreSQL")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run control-server migrations")?;

    let master_key = parse_key(&required("REMOTEX_CONTROL_MASTER_KEY_HEX")?)?;
    let bind: SocketAddr = std::env::var("REMOTEX_CONTROL_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
        .parse()
        .context("parse REMOTEX_CONTROL_BIND")?;
    let service = ControlService::new(
        Arc::new(PostgresRepository::new(pool)),
        master_key,
        ControlConfig {
            relay_address: required("REMOTEX_PUBLIC_RELAY_ADDRESS")?,
            relay_server_name: required("REMOTEX_RELAY_SERVER_NAME")?,
            device_offline_after_ms: parse_or(
                "REMOTEX_DEVICE_OFFLINE_AFTER_MS",
                remotex_control::DEFAULT_DEVICE_OFFLINE_AFTER_MS,
            )?,
            session_lifetime_ms: parse_or(
                "REMOTEX_SESSION_LIFETIME_MS",
                remotex_control::DEFAULT_SESSION_LIFETIME_MS,
            )?,
        },
    );
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .context("bind control server")?;
    info!(address = %listener.local_addr()?, "control server listening");
    axum::serve(listener, router(ApiState { service }))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serve control API")
}

fn required(name: &str) -> anyhow::Result<String> {
    let file_name = format!("{name}_FILE");
    match (std::env::var(name), std::env::var(&file_name)) {
        (Ok(_), Ok(_)) => anyhow::bail!("set only one of {name} and {file_name}"),
        (Ok(value), Err(std::env::VarError::NotPresent)) => Ok(value),
        (Err(std::env::VarError::NotPresent), Ok(path)) => read_secret(name, &path),
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => {
            anyhow::bail!("required environment variable {name} or {file_name} is missing")
        }
        (Err(error), _) => Err(error).with_context(|| format!("read {name}")),
        (_, Err(error)) => Err(error).with_context(|| format!("read {file_name}")),
    }
}

fn read_secret(name: &str, path: &str) -> anyhow::Result<String> {
    let metadata = std::fs::metadata(path).with_context(|| format!("inspect {name} file"))?;
    if metadata.len() > 16 * 1024 {
        anyhow::bail!("{name} file exceeds 16 KiB");
    }
    let value = std::fs::read_to_string(path).with_context(|| format!("read {name} file"))?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        anyhow::bail!("{name} file is empty");
    }
    Ok(value)
}

fn parse_or<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| anyhow::anyhow!("parse {name}: {error}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("read {name}")),
    }
}

fn parse_key(value: &str) -> anyhow::Result<[u8; 32]> {
    hex::decode(value)
        .context("decode 64-character control master key")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("control master key must contain exactly 32 bytes"))
}
