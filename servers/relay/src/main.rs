//! `RemoteX` M1 relay composition root.

use anyhow::{Context, bail};
use quinn::ServerConfig;
use remotex_protocol::{Role, SessionId, SessionToken};
use remotex_relay::{RelayLimits, RelayServer, SessionAuthorizer};
use std::{fs::File, io::BufReader, net::SocketAddr, path::Path, time::Duration};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?;

    let bind: SocketAddr = required("REMOTEX_RELAY_BIND")?
        .parse()
        .context("parse REMOTEX_RELAY_BIND")?;
    let certificate_path = required("REMOTEX_RELAY_CERT")?;
    let private_key_path = required("REMOTEX_RELAY_KEY")?;
    let session_id: SessionId = required("REMOTEX_SESSION_ID")?
        .parse()
        .context("parse REMOTEX_SESSION_ID")?;
    let controller_token = parse_token(&required("REMOTEX_CONTROLLER_TOKEN_HEX")?)?;
    let agent_token = parse_token(&required("REMOTEX_AGENT_TOKEN_HEX")?)?;
    let lifetime_seconds: u64 = std::env::var("REMOTEX_TOKEN_LIFETIME_SECONDS")
        .unwrap_or_else(|_| "300".to_owned())
        .parse()
        .context("parse REMOTEX_TOKEN_LIFETIME_SECONDS")?;
    let expires_at_ms = now_ms()?
        .checked_add(lifetime_seconds.saturating_mul(1000))
        .context("token expiration overflow")?;

    let authorizer = SessionAuthorizer::default();
    authorizer
        .grant(
            session_id,
            Role::Controller,
            controller_token,
            expires_at_ms,
        )
        .await;
    authorizer
        .grant(session_id, Role::Agent, agent_token, expires_at_ms)
        .await;

    let server_config =
        load_server_config(Path::new(&certificate_path), Path::new(&private_key_path))?;
    let endpoint = quinn::Endpoint::server(server_config, bind).context("bind relay endpoint")?;
    info!(address = %endpoint.local_addr()?, %session_id, "relay listening");
    RelayServer::new(
        authorizer,
        RelayLimits {
            idle_timeout: Duration::from_secs(30),
            ..RelayLimits::default()
        },
    )
    .serve_until(endpoint, async {
        let _result = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("required environment variable {name} is missing"))
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
