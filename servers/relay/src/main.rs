//! `RemoteX` M1 relay composition root.

use anyhow::{Context, bail};
use axum::{Json, Router, routing::get};
use quinn::ServerConfig;
use remotex_protocol::{Role, SessionId, SessionToken};
use remotex_relay::{
    PostgresSessionAuthenticator, RelayLimits, RelayServer, SessionAuthenticator, SessionAuthorizer,
};
use sqlx::postgres::PgPoolOptions;
use std::{
    fmt::Display, fs::File, io::BufReader, net::SocketAddr, path::Path, str::FromStr, sync::Arc,
    time::Duration,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(serde::Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?;

    let bind: SocketAddr = required("REMOTEX_RELAY_BIND")?
        .parse()
        .context("parse REMOTEX_RELAY_BIND")?;
    let certificate_path = required("REMOTEX_RELAY_CERT")?;
    let private_key_path = required("REMOTEX_RELAY_KEY")?;
    let (authorizer, development_session) = load_authenticator().await?;

    let server_config =
        load_server_config(Path::new(&certificate_path), Path::new(&private_key_path))?;
    let endpoint = quinn::Endpoint::server(server_config, bind).context("bind relay endpoint")?;
    let health_bind: SocketAddr = std::env::var("REMOTEX_RELAY_HEALTH_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8081".to_owned())
        .parse()
        .context("parse REMOTEX_RELAY_HEALTH_BIND")?;
    let health_listener = tokio::net::TcpListener::bind(health_bind)
        .await
        .context("bind Relay health server")?;
    let health_task = tokio::spawn(async move {
        axum::serve(
            health_listener,
            Router::new().route(
                "/health",
                get(|| async { Json(HealthResponse { status: "ok" }) }),
            ),
        )
        .await
    });
    info!(address = %endpoint.local_addr()?, ?development_session, "relay listening");
    let defaults = RelayLimits::default();
    let result = RelayServer::with_authenticator(
        authorizer,
        RelayLimits {
            maximum_frame_size: parse_or(
                "REMOTEX_RELAY_MAX_MESSAGE_SIZE",
                defaults.maximum_frame_size,
            )?,
            max_connections: parse_or("REMOTEX_RELAY_MAX_CONNECTIONS", defaults.max_connections)?,
            max_pending_sessions: parse_or(
                "REMOTEX_RELAY_MAX_PENDING_SESSIONS",
                defaults.max_pending_sessions,
            )?,
            outbound_queue_capacity: parse_or(
                "REMOTEX_RELAY_QUEUE_CAPACITY",
                defaults.outbound_queue_capacity,
            )?,
            handshake_timeout: Duration::from_millis(parse_or(
                "REMOTEX_RELAY_HANDSHAKE_TIMEOUT_MS",
                u64::try_from(defaults.handshake_timeout.as_millis())?,
            )?),
            heartbeat_interval: Duration::from_millis(parse_or(
                "REMOTEX_RELAY_HEARTBEAT_INTERVAL_MS",
                u64::try_from(defaults.heartbeat_interval.as_millis())?,
            )?),
            peer_timeout: Duration::from_millis(parse_or(
                "REMOTEX_RELAY_PEER_TIMEOUT_MS",
                u64::try_from(defaults.peer_timeout.as_millis())?,
            )?),
        },
    )
    .serve_until(endpoint, async {
        let _result = tokio::signal::ctrl_c().await;
    })
    .await;
    health_task.abort();
    result?;
    Ok(())
}

async fn load_authenticator() -> anyhow::Result<(Arc<dyn SessionAuthenticator>, Option<SessionId>)>
{
    if let Some(database_url) = optional("REMOTEX_DATABASE_URL")? {
        let pool = PgPoolOptions::new()
            .max_connections(parse_or("REMOTEX_RELAY_DB_CONNECTIONS", 10_u32)?)
            .connect(&database_url)
            .await
            .context("connect Relay to PostgreSQL")?;
        sqlx::query("SELECT 1")
            .execute(&pool)
            .await
            .context("check Relay PostgreSQL readiness")?;
        return Ok((Arc::new(PostgresSessionAuthenticator::new(pool)), None));
    }

    let session_id: SessionId = required("REMOTEX_SESSION_ID")?
        .parse()
        .context("parse REMOTEX_SESSION_ID")?;
    let controller_token = parse_token(&required("REMOTEX_CONTROLLER_TOKEN_HEX")?)?;
    let agent_token = parse_token(&required("REMOTEX_AGENT_TOKEN_HEX")?)?;
    let lifetime_seconds: u64 = parse_or("REMOTEX_TOKEN_LIFETIME_SECONDS", 300)?;
    let expires_at_ms = now_ms()?
        .checked_add(lifetime_seconds.saturating_mul(1000))
        .context("token expiration overflow")?;
    let development = SessionAuthorizer::default();
    development
        .grant(
            session_id,
            Role::Controller,
            controller_token,
            expires_at_ms,
        )
        .await;
    development
        .grant(session_id, Role::Agent, agent_token, expires_at_ms)
        .await;
    Ok((Arc::new(development), Some(session_id)))
}

fn required(name: &str) -> anyhow::Result<String> {
    optional(name)?.with_context(|| {
        let file_name = format!("{name}_FILE");
        format!("required environment variable {name} or {file_name} is missing")
    })
}

fn optional(name: &str) -> anyhow::Result<Option<String>> {
    let file_name = format!("{name}_FILE");
    match (std::env::var(name), std::env::var(&file_name)) {
        (Ok(_), Ok(_)) => anyhow::bail!("set only one of {name} and {file_name}"),
        (Ok(value), Err(std::env::VarError::NotPresent)) => Ok(Some(value)),
        (Err(std::env::VarError::NotPresent), Ok(path)) => read_secret(name, &path).map(Some),
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => Ok(None),
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
    T: FromStr,
    T::Err: Display,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| anyhow::anyhow!("parse {name}: {error}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(anyhow::anyhow!("read {name}: {error}")),
    }
}

fn parse_token(value: &str) -> anyhow::Result<SessionToken> {
    let decoded = hex::decode(value).context("decode 64-character token hex")?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("session token must contain exactly 32 bytes"))?;
    Ok(SessionToken::from_bytes(bytes))
}

fn load_server_config(certificate: &Path, private_key: &Path) -> anyhow::Result<ServerConfig> {
    let mut certificate_reader = BufReader::new(
        File::open(certificate)
            .with_context(|| format!("open certificate {}", certificate.display()))?,
    );
    let certificates = rustls_pemfile::certs(&mut certificate_reader)
        .collect::<Result<Vec<_>, _>>()
        .context("read PEM certificate chain")?;
    if certificates.is_empty() {
        bail!("certificate file contains no certificates");
    }

    let mut key_reader = BufReader::new(
        File::open(private_key)
            .with_context(|| format!("open private key {}", private_key.display()))?,
    );
    let key = rustls_pemfile::private_key(&mut key_reader)
        .context("read PEM private key")?
        .context("private-key file contains no supported key")?;
    ServerConfig::with_single_cert(certificates, key).context("build relay TLS configuration")
}

fn now_ms() -> anyhow::Result<u64> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    u64::try_from(duration.as_millis()).context("system timestamp is out of range")
}
