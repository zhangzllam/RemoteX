//! `RemoteX` M6 Windows remote-screen, input, and plain-text clipboard Agent.

#[cfg(windows)]
use anyhow::Context;
#[cfg(windows)]
use bytes::Bytes;
#[cfg(windows)]
use quinn::{ClientConfig, Endpoint};
#[cfg(windows)]
use remotex_capture::{CaptureError, DxgiCapture, MonitorId, MonitorInfo, ScreenCapture};
#[cfg(windows)]
use remotex_clipboard::{ClipboardError, PermissionedClipboard, WindowsClipboardBackend};
#[cfg(windows)]
use remotex_crypto::{SessionCipher, SessionDirection, XChaChaSessionCipher};
#[cfg(windows)]
use remotex_input::{
    DisplayGeometry, InputController, InputError, PermissionedInputController, WindowsInputBackend,
};
#[cfg(windows)]
use remotex_protocol::{
    ClipboardOrigin, DisplayId, MAX_CLIPBOARD_TEXT_SIZE, Message, MessageEnvelope,
    RelayClientMessage, RelayServerMessage, Role, SessionId, SessionToken, decode_wire,
    encode_wire,
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
struct AgentConfig {
    relay_address: SocketAddr,
    server_name: String,
    certificate_path: String,
    session_id: SessionId,
    token: SessionToken,
    end_to_end_key: [u8; 32],
    frames_per_second: u32,
    input_permission: bool,
    clipboard_permission: bool,
}

#[cfg(windows)]
struct AgentSessionContext<'a> {
    session_id: SessionId,
    frames_per_second: u32,
    outbound_cipher: &'a XChaChaSessionCipher,
    inbound_cipher: &'a XChaChaSessionCipher,
}

#[cfg(windows)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?;

    let AgentConfig {
        relay_address,
        server_name,
        certificate_path,
        session_id,
        token,
        end_to_end_key,
        frames_per_second,
        input_permission,
        clipboard_permission,
    } = load_config()?;
    let outbound_cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let mut capture = DxgiCapture::new();
    let monitor = start_capture(&mut capture)?;
    let input_backend = WindowsInputBackend::new(DisplayGeometry {
        id: DisplayId::new(monitor.id.0.clone())?,
        origin_x: monitor.origin_x,
        origin_y: monitor.origin_y,
        width: monitor.width,
        height: monitor.height,
    })?;
    let mut input = PermissionedInputController::new(input_backend, input_permission);
    let inbound_cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::ControllerToAgent,
    );
    info!(
        event = "session_permissions_configured",
        control_input = input_permission,
        clipboard = clipboard_permission,
        "local M6 permissions loaded"
    );
    let mut clipboard = PermissionedClipboard::new(
        WindowsClipboardBackend,
        ClipboardOrigin::Agent,
        clipboard_permission,
    );

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
    info!(%session_id, %relay_address, "remote session active");

    run_active_session(
        &mut transport,
        &mut capture,
        &mut input,
        &mut clipboard,
        AgentSessionContext {
            session_id,
            frames_per_second,
            outbound_cipher: &outbound_cipher,
            inbound_cipher: &inbound_cipher,
        },
    )
    .await?;
    input.release_all()?;
    let _result = transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::Close)?))
        .await;
    capture.stop()?;
    transport.close().await?;
    client_endpoint.close(0_u32.into(), b"agent stopped");
    Ok(())
}

#[cfg(windows)]
async fn run_active_session(
    transport: &mut QuicFrameConnection,
    capture: &mut DxgiCapture,
    input: &mut impl InputController,
    clipboard: &mut PermissionedClipboard<WindowsClipboardBackend>,
    context: AgentSessionContext<'_>,
) -> anyhow::Result<()> {
    let codec = SoftwareEncoder::new(EncoderConfig::default())?;
    let mut outbound_sequence = 0_u64;
    let mut expected_inbound_sequence = 0_u64;
    let mut capture_interval = tokio::time::interval(Duration::from_millis(
        1000 / u64::from(context.frames_per_second),
    ));
    capture_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut clipboard_interval = tokio::time::interval(Duration::from_millis(500));
    clipboard_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = capture_interval.tick() => {
                let frame = match capture.next_frame() {
                    Ok(frame) => frame,
                    Err(CaptureError::Timeout) => continue,
                    Err(error) => return Err(error.into()),
                };
                let video = codec.encode(&frame)?;
                send_agent_message(
                    transport,
                    context.session_id,
                    context.outbound_cipher,
                    &mut outbound_sequence,
                    frame.timestamp_ms,
                    Message::Video(video),
                ).await?;
            }
            _ = clipboard_interval.tick(), if clipboard.is_enabled() => {
                match clipboard.poll() {
                    Ok(Some(message)) => {
                        send_agent_message(
                            transport,
                            context.session_id,
                            context.outbound_cipher,
                            &mut outbound_sequence,
                            now_ms()?,
                            Message::Clipboard(message),
                        ).await?;
                    }
                    Ok(None) => {}
                    Err(error) => warn!(event = "clipboard_poll_failed", %error),
                }
            }
            incoming = transport.receive() => {
                handle_relay_message(
                    transport,
                    decode_wire(&incoming?)?,
                    context.session_id,
                    context.inbound_cipher,
                    &mut expected_inbound_sequence,
                    input,
                    clipboard,
                ).await?;
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
async fn send_agent_message(
    transport: &mut QuicFrameConnection,
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    sequence: &mut u64,
    timestamp_ms: u64,
    message: Message,
) -> anyhow::Result<()> {
    let envelope = MessageEnvelope::new(session_id, *sequence, timestamp_ms, message);
    let plaintext = encode_wire(&envelope).context("encode Agent envelope")?;
    let ciphertext = cipher
        .seal(*sequence, &plaintext)
        .context("encrypt Agent envelope")?;
    let mut payload = Vec::with_capacity(8 + ciphertext.len());
    payload.extend_from_slice(&(*sequence).to_be_bytes());
    payload.extend_from_slice(&ciphertext);
    transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::Payload(
            payload,
        ))?))
        .await?;
    *sequence = sequence
        .checked_add(1)
        .context("Agent outbound sequence space exhausted")?;
    Ok(())
}

#[cfg(windows)]
fn load_config() -> anyhow::Result<AgentConfig> {
    let relay_address = required("REMOTEX_RELAY_ADDRESS")?
        .parse()
        .context("parse REMOTEX_RELAY_ADDRESS")?;
    let session_id = required("REMOTEX_SESSION_ID")?
        .parse()
        .context("parse REMOTEX_SESSION_ID")?;
    let frames_per_second = std::env::var("REMOTEX_VIDEO_FPS")
        .unwrap_or_else(|_| "12".to_owned())
        .parse()
        .context("parse REMOTEX_VIDEO_FPS")?;
    if !(1..=30).contains(&frames_per_second) {
        anyhow::bail!("REMOTEX_VIDEO_FPS must be between 1 and 30");
    }
    Ok(AgentConfig {
        relay_address,
        server_name: required("REMOTEX_RELAY_SERVER_NAME")?,
        certificate_path: required("REMOTEX_RELAY_CA_CERT")?,
        session_id,
        token: parse_token(&required("REMOTEX_AGENT_TOKEN_HEX")?)?,
        end_to_end_key: parse_key(&required("REMOTEX_E2E_KEY_HEX")?)?,
        frames_per_second,
        input_permission: parse_switch("REMOTEX_ALLOW_INPUT", false)?,
        clipboard_permission: parse_switch("REMOTEX_ALLOW_CLIPBOARD", false)?,
    })
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
            RelayServerMessage::Heartbeat { nonce } => {
                let acknowledgement = RelayClientMessage::HeartbeatAck { nonce };
                transport
                    .send(Bytes::from(encode_wire(&acknowledgement)?))
                    .await?;
            }
            RelayServerMessage::HeartbeatAck { .. } => {}
            RelayServerMessage::SessionClosed { reason } => {
                anyhow::bail!("relay session closed before pairing: {reason:?}");
            }
            RelayServerMessage::ProtocolError { code, message } => {
                anyhow::bail!("relay protocol error {code:?}: {message}");
            }
            RelayServerMessage::Payload(_) => {
                anyhow::bail!("relay delivered a payload before PeerReady");
            }
        }
    }
}

#[cfg(windows)]
async fn handle_relay_message(
    transport: &mut QuicFrameConnection,
    message: RelayServerMessage,
    session_id: SessionId,
    inbound_cipher: &XChaChaSessionCipher,
    expected_sequence: &mut u64,
    input: &mut impl InputController,
    clipboard: &mut PermissionedClipboard<WindowsClipboardBackend>,
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
        RelayServerMessage::Payload(payload) => {
            apply_controller_payload(
                &payload,
                session_id,
                inbound_cipher,
                expected_sequence,
                input,
                clipboard,
            )?;
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
fn apply_controller_payload(
    payload: &[u8],
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    expected_sequence: &mut u64,
    input: &mut impl InputController,
    clipboard: &mut PermissionedClipboard<WindowsClipboardBackend>,
) -> anyhow::Result<()> {
    if payload.len() > MAX_CLIPBOARD_TEXT_SIZE + 1024 {
        anyhow::bail!("Controller data payload exceeds the M6 input/clipboard limit");
    }
    if payload.len() < 8 {
        anyhow::bail!("encrypted Controller payload is missing its sequence number");
    }
    let sequence = u64::from_be_bytes(
        payload[..8]
            .try_into()
            .context("read encrypted Controller sequence")?,
    );
    if sequence != *expected_sequence {
        anyhow::bail!(
            "unexpected Controller sequence {sequence}; expected {}",
            *expected_sequence
        );
    }
    let plaintext = cipher
        .open(sequence, &payload[8..])
        .context("authenticate and decrypt Controller envelope")?;
    let envelope: MessageEnvelope =
        decode_wire(&plaintext).context("decode Controller envelope")?;
    envelope.validate()?;
    if envelope.session_id != session_id || envelope.sequence != sequence {
        anyhow::bail!("Controller envelope Session or sequence mismatch");
    }
    match envelope.message {
        Message::Input(event) => match input.apply(event) {
            Ok(()) => {}
            Err(InputError::PermissionDenied) => {
                warn!(event = "input_permission_denied", %session_id);
            }
            Err(error) => return Err(error.into()),
        },
        Message::Clipboard(message) => match clipboard.apply(message) {
            Ok(_) => {}
            Err(ClipboardError::PermissionDenied) => {
                warn!(event = "clipboard_permission_denied", %session_id);
            }
            Err(error) => return Err(error.into()),
        },
        _ => {
            anyhow::bail!("Controller sent a message not allowed in its data direction");
        }
    }
    *expected_sequence = expected_sequence
        .checked_add(1)
        .context("Controller inbound sequence space exhausted")?;
    Ok(())
}

#[cfg(windows)]
fn start_capture(capture: &mut DxgiCapture) -> anyhow::Result<MonitorInfo> {
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
    Ok(monitor.clone())
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

#[cfg(windows)]
fn parse_switch(name: &str, default: bool) -> anyhow::Result<bool> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => anyhow::bail!("{name} must be true/false, yes/no, on/off, or 1/0"),
        },
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("read {name}")),
    }
}

#[cfg(windows)]
fn now_ms() -> anyhow::Result<u64> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    u64::try_from(duration.as_millis()).context("system timestamp is out of range")
}

#[cfg(not(windows))]
fn main() {
    eprintln!("the M6 desktop agent currently supports Windows only");
}
