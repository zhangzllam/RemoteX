//! `RemoteX` M3 Windows capture and remote-video sender.

#[cfg(windows)]
use anyhow::Context;
#[cfg(windows)]
use bytes::Bytes;
#[cfg(windows)]
use quinn::{ClientConfig, Endpoint};
#[cfg(windows)]
use remotex_capture::{CaptureError, DxgiCapture, MonitorId, ScreenCapture};
#[cfg(windows)]
use remotex_crypto::{SessionCipher, SessionDirection, XChaChaSessionCipher};
#[cfg(windows)]
use remotex_protocol::{
    Message, MessageEnvelope, RelayClientMessage, RelayServerMessage, Role, SessionId,
    SessionToken, decode_wire, encode_wire,
};
#[cfg(windows)]
use remotex_transport::{Connection, DEFAULT_MAX_FRAME_SIZE, QuicFrameConnection};
#[cfg(windows)]
use remotex_video::{EncoderConfig, SoftwareEncoder};
#[cfg(windows)]
use rustls::RootCertStore;
#[cfg(windows)]
use std::{fs::File, io::BufReader, net::SocketAddr, path::Path, sync::Arc, time::Duration};
#[cfg(windows)]
use tokio::time::MissedTickBehavior;
#[cfg(windows)]
use tracing::{info, warn};
#[cfg(windows)]
use tracing_subscriber::EnvFilter;

#[cfg(windows)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?;

    let relay_address: SocketAddr = required("REMOTEX_RELAY_ADDRESS")?
        .parse()
        .context("parse REMOTEX_RELAY_ADDRESS")?;
    let server_name = required("REMOTEX_RELAY_SERVER_NAME")?;
    let certificate_path = required("REMOTEX_RELAY_CA_CERT")?;
    let session_id: SessionId = required("REMOTEX_SESSION_ID")?
        .parse()
        .context("parse REMOTEX_SESSION_ID")?;
    let token = parse_token(&required("REMOTEX_AGENT_TOKEN_HEX")?)?;
    let end_to_end_key = parse_key(&required("REMOTEX_E2E_KEY_HEX")?)?;
    let cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let frames_per_second: u32 = std::env::var("REMOTEX_VIDEO_FPS")
        .unwrap_or_else(|_| "12".to_owned())
        .parse()
        .context("parse REMOTEX_VIDEO_FPS")?;
    if !(1..=30).contains(&frames_per_second) {
        anyhow::bail!("REMOTEX_VIDEO_FPS must be between 1 and 30");
    }

    let mut capture = DxgiCapture::new();
    start_capture(&mut capture)?;

    let client_endpoint = client_endpoint(Path::new(&certificate_path))?;
    let connection = client_endpoint
        .connect(relay_address, &server_name)
        .context("create relay connection")?
        .await
        .context("connect to relay")?;
    let (send, receive) = connection.open_bi().await.context("open relay stream")?;
    let mut transport = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
    let hello = RelayClientMessage::ClientHello(remotex_protocol::ClientHello::new(
        session_id,
        Role::Agent,
        token,
    ));
    transport.send(Bytes::from(encode_wire(&hello)?)).await?;
    wait_for_peer(&mut transport, session_id).await?;
    info!(%session_id, %relay_address, "video relay paired");

    let codec = SoftwareEncoder::new(EncoderConfig::default())?;
    let mut sequence = 0_u64;
    let mut interval =
        tokio::time::interval(Duration::from_millis(1000 / u64::from(frames_per_second)));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        let capture_tick = tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => true,
            incoming = transport.receive() => {
                handle_relay_message(&mut transport, decode_wire(&incoming?)?).await?;
                false
            }
        };
        if !capture_tick {
            continue;
        }
        let frame = match capture.next_frame() {
            Ok(frame) => frame,
            Err(CaptureError::Timeout) => continue,
            Err(error) => return Err(error.into()),
        };
        let video = codec.encode(&frame)?;
        let envelope = MessageEnvelope::new(
            session_id,
            sequence,
            frame.timestamp_ms,
            Message::Video(video),
        );
        let serialized_frame = encode_wire(&envelope).context("encode video envelope")?;
        let encrypted_frame = cipher
            .seal(sequence, &serialized_frame)
            .context("encrypt video envelope")?;
        let mut wire_frame = Vec::with_capacity(8 + encrypted_frame.len());
        wire_frame.extend_from_slice(&sequence.to_be_bytes());
        wire_frame.extend_from_slice(&encrypted_frame);
        let relay_frame = RelayClientMessage::Payload(wire_frame);
        if let Err(error) = transport
            .send(Bytes::from(encode_wire(&relay_frame)?))
            .await
        {
            warn!(%error, "video relay disconnected");
            return Err(error.into());
        }
        sequence = sequence
            .checked_add(1)
            .context("video sequence space exhausted")?;
    }
    let _result = transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::Close)?))
        .await;
    capture.stop()?;
    transport.close().await?;
    client_endpoint.close(0_u32.into(), b"agent stopped");
    Ok(())
}

#[cfg(windows)]
async fn wait_for_peer(
    transport: &mut QuicFrameConnection,
    session_id: SessionId,
) -> anyhow::Result<()> {
    loop {
        let message: RelayServerMessage = decode_wire(&transport.receive().await?)?;
        match message {
            RelayServerMessage::WaitingForPeer { role } => {
                info!(%session_id, ?role, "waiting for relay peer");
            }
            RelayServerMessage::PeerReady => return Ok(()),
            other => handle_relay_message(transport, other).await?,
        }
    }
}

#[cfg(windows)]
async fn handle_relay_message(
    transport: &mut QuicFrameConnection,
    message: RelayServerMessage,
) -> anyhow::Result<()> {
    match message {
        RelayServerMessage::Heartbeat { nonce } => {
            let acknowledgement = RelayClientMessage::HeartbeatAck { nonce };
            transport
                .send(Bytes::from(encode_wire(&acknowledgement)?))
                .await?;
        }
        RelayServerMessage::HeartbeatAck { .. }
        | RelayServerMessage::WaitingForPeer { .. }
        | RelayServerMessage::PeerReady => {}
        RelayServerMessage::Payload(_) => {
            warn!("M3 Agent ignored an unsupported controller payload");
        }
        RelayServerMessage::SessionClosed { reason } => {
            anyhow::bail!("relay session closed: {reason:?}");
        }
        RelayServerMessage::ProtocolError { code, message } => {
            anyhow::bail!("relay protocol error {code:?}: {message}");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn start_capture(capture: &mut DxgiCapture) -> anyhow::Result<()> {
    let monitors = capture.monitors()?;
    let requested_monitor = std::env::var("REMOTEX_MONITOR_ID").ok().map(MonitorId);
    let monitor = requested_monitor
        .as_ref()
        .and_then(|id| monitors.iter().find(|monitor| monitor.id == *id))
        .or_else(|| monitors.iter().find(|monitor| monitor.is_primary))
        .or_else(|| monitors.first())
        .context("no attached monitor is available")?;
    capture.start(&monitor.id)?;
    info!(monitor = %monitor.name, width = monitor.width, height = monitor.height, "capture started");
    Ok(())
}

#[cfg(windows)]
fn client_endpoint(certificate_path: &Path) -> anyhow::Result<Endpoint> {
    let mut reader =
        BufReader::new(File::open(certificate_path).with_context(|| {
            format!("open relay CA certificate {}", certificate_path.display())
        })?);
    let certificates = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .context("read relay CA certificate")?;
    if certificates.is_empty() {
        anyhow::bail!("relay CA certificate file contains no certificates");
    }
    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots
            .add(certificate)
            .context("trust relay CA certificate")?;
    }
    let config = ClientConfig::with_root_certificates(Arc::new(roots))?;
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

#[cfg(windows)]
fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

#[cfg(windows)]
fn parse_token(value: &str) -> anyhow::Result<SessionToken> {
    let decoded = hex::decode(value).context("decode 64-character token hex")?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("session token must contain exactly 32 bytes"))?;
    Ok(SessionToken::from_bytes(bytes))
}

#[cfg(windows)]
fn parse_key(value: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = hex::decode(value).context("decode 64-character end-to-end key hex")?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("end-to-end key must contain exactly 32 bytes"))
}

#[cfg(not(windows))]
fn main() {
    eprintln!("the M3 desktop agent currently supports Windows only");
}
