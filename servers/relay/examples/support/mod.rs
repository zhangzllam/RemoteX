use anyhow::Context;
use quinn::{ClientConfig, Endpoint};
use remotex_protocol::{
    ClientHello, RelayClientMessage, RelayServerMessage, Role, SessionId, SessionToken,
    decode_wire, encode_wire,
};
use remotex_transport::{DEFAULT_MAX_FRAME_SIZE, read_frame, write_frame};
use rustls::RootCertStore;
use std::{
    fs::File,
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tracing::info;

pub struct MockPeer {
    endpoint: Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    receive: quinn::RecvStream,
    maximum_frame_size: usize,
}

impl MockPeer {
    pub async fn connect(role: Role) -> anyhow::Result<Self> {
        let relay_address: SocketAddr = required("REMOTEX_RELAY_ADDRESS")?
            .parse()
            .context("parse REMOTEX_RELAY_ADDRESS")?;
        let server_name = required("REMOTEX_RELAY_SERVER_NAME")?;
        let ca_certificate = PathBuf::from(required("REMOTEX_RELAY_CA_CERT")?);
        let session_id: SessionId = required("REMOTEX_SESSION_ID")?
            .parse()
            .context("parse REMOTEX_SESSION_ID")?;
        let token_name = match role {
            Role::Controller => "REMOTEX_CONTROLLER_TOKEN_HEX",
            Role::Agent => "REMOTEX_AGENT_TOKEN_HEX",
        };
        let token = parse_token(&required(token_name)?)?;
        let maximum_frame_size = std::env::var("REMOTEX_RELAY_MAX_MESSAGE_SIZE").map_or(
            Ok(DEFAULT_MAX_FRAME_SIZE),
            |value| {
                value
                    .parse()
                    .context("parse REMOTEX_RELAY_MAX_MESSAGE_SIZE")
            },
        )?;

        let endpoint = client_endpoint(&ca_certificate)?;
        let connection = endpoint
            .connect(relay_address, &server_name)
            .context("create relay connection")?
            .await
            .context("connect to relay")?;
        let (mut send, receive) = connection.open_bi().await.context("open relay stream")?;
        let hello = RelayClientMessage::ClientHello(ClientHello::new(session_id, role, token));
        write_frame(&mut send, &encode_wire(&hello)?, maximum_frame_size).await?;
        let mut peer = Self {
            endpoint,
            connection,
            send,
            receive,
            maximum_frame_size,
        };
        peer.wait_until_ready(session_id, role).await?;
        Ok(peer)
    }

    pub async fn send_payload(&mut self, payload: Vec<u8>) -> anyhow::Result<()> {
        self.send_message(&RelayClientMessage::Payload(payload))
            .await
    }

    pub async fn receive_payload(&mut self) -> anyhow::Result<Vec<u8>> {
        loop {
            match self.receive_message().await? {
                RelayServerMessage::Payload(payload) => return Ok(payload),
                RelayServerMessage::Heartbeat { nonce } => {
                    self.send_message(&RelayClientMessage::HeartbeatAck { nonce })
                        .await?;
                }
                RelayServerMessage::HeartbeatAck { .. }
                | RelayServerMessage::WaitingForPeer { .. }
                | RelayServerMessage::PeerReady
                | RelayServerMessage::PathControl(_) => {}
                RelayServerMessage::SessionClosed { reason } => {
                    anyhow::bail!("relay session closed: {reason:?}");
                }
                RelayServerMessage::ProtocolError { code, message } => {
                    anyhow::bail!("relay protocol error {code:?}: {message}");
                }
            }
        }
    }

    pub async fn close(mut self) -> anyhow::Result<()> {
        let _result = self.send_message(&RelayClientMessage::Close).await;
        let _result = self.send.finish();
        let _result = tokio::time::timeout(Duration::from_secs(1), self.send.stopped()).await;
        self.connection.close(0_u32.into(), b"mock client finished");
        self.endpoint.close(0_u32.into(), b"mock client finished");
        Ok(())
    }

    async fn wait_until_ready(&mut self, session_id: SessionId, role: Role) -> anyhow::Result<()> {
        loop {
            match self.receive_message().await? {
                RelayServerMessage::WaitingForPeer { role: missing_role } => {
                    info!(%session_id, ?role, ?missing_role, "mock peer waiting");
                }
                RelayServerMessage::PeerReady => {
                    info!(%session_id, ?role, "mock peer ready");
                    return Ok(());
                }
                RelayServerMessage::Heartbeat { nonce } => {
                    self.send_message(&RelayClientMessage::HeartbeatAck { nonce })
                        .await?;
                }
                RelayServerMessage::HeartbeatAck { .. } => {}
                RelayServerMessage::PathControl(_) => {
                    anyhow::bail!("relay delivered path control before PeerReady");
                }
                RelayServerMessage::SessionClosed { reason } => {
                    anyhow::bail!("relay session closed before pairing: {reason:?}");
                }
                RelayServerMessage::ProtocolError { code, message } => {
                    anyhow::bail!("relay protocol error {code:?}: {message}");
                }
                RelayServerMessage::Payload(_) => {
                    anyhow::bail!("relay delivered payload before PeerReady");
                }
            }
        }
    }

    async fn send_message(&mut self, message: &RelayClientMessage) -> anyhow::Result<()> {
        write_frame(
            &mut self.send,
            &encode_wire(message)?,
            self.maximum_frame_size,
        )
        .await?;
        Ok(())
    }

    async fn receive_message(&mut self) -> anyhow::Result<RelayServerMessage> {
        let bytes = read_frame(&mut self.receive, self.maximum_frame_size).await?;
        Ok(decode_wire(&bytes)?)
    }
}

pub fn initialize_tracing() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))
}

pub fn now_ms() -> anyhow::Result<u64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before Unix epoch")?;
    u64::try_from(elapsed.as_millis()).context("system timestamp is out of range")
}

fn client_endpoint(certificate_path: &Path) -> anyhow::Result<Endpoint> {
    let mut reader =
        BufReader::new(File::open(certificate_path).with_context(|| {
            format!("open relay CA certificate {}", certificate_path.display())
        })?);
    let certificates = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .context("read relay CA certificate")?;
    if certificates.is_empty() {
        anyhow::bail!("relay CA certificate contains no certificates");
    }
    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate).context("trust relay certificate")?;
    }
    let config = ClientConfig::with_root_certificates(Arc::new(roots))?;
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

fn parse_token(value: &str) -> anyhow::Result<SessionToken> {
    let decoded = hex::decode(value).context("decode 64-character session token")?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("session token must contain exactly 32 bytes"))?;
    Ok(SessionToken::from_bytes(bytes))
}
