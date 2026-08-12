//! `RemoteX` M5 Tauri Controller for remote video and permissioned mouse/keyboard input.

use anyhow::Context;
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use quinn::{ClientConfig, Endpoint};
use remotex_crypto::{SessionCipher, SessionDirection, XChaChaSessionCipher};
use remotex_input::normalize_unit_coordinate;
use remotex_protocol::{
    DisplayId, InputEvent, KeyCode, Message, MessageEnvelope, MouseButton, RelayClientMessage,
    RelayServerMessage, Role, SessionId, SessionToken, VideoCodec, WheelAxis, decode_wire,
    encode_wire,
};
use remotex_transport::{Connection, DEFAULT_MAX_FRAME_SIZE, QuicFrameConnection};
use rustls::RootCertStore;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::BufReader, net::SocketAddr, path::Path, sync::Arc};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{mpsc, oneshot};

const INPUT_QUEUE_CAPACITY: usize = 128;

#[derive(Default)]
struct StreamControl {
    active: std::sync::Mutex<Option<ActiveStream>>,
}

struct ActiveStream {
    cancellation: oneshot::Sender<()>,
    input: mpsc::Sender<InputEvent>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectRequest {
    relay_address: String,
    server_name: String,
    ca_certificate_path: String,
    session_id: String,
    token_hex: String,
    end_to_end_key_hex: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoFrameEvent {
    sequence: u64,
    width: u32,
    height: u32,
    source_timestamp_ms: u64,
    mime_type: &'static str,
    data: String,
}

#[derive(Clone, Debug, Serialize)]
struct StatusEvent {
    state: &'static str,
    message: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum MouseButtonRequest {
    Left,
    Right,
    Middle,
}

impl From<MouseButtonRequest> for MouseButton {
    fn from(value: MouseButtonRequest) -> Self {
        match value {
            MouseButtonRequest::Left => Self::Left,
            MouseButtonRequest::Right => Self::Right,
            MouseButtonRequest::Middle => Self::Middle,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum MouseInputRequest {
    Move {
        display_id: Option<String>,
        x: f64,
        y: f64,
    },
    ButtonDown {
        button: MouseButtonRequest,
        x: f64,
        y: f64,
    },
    ButtonUp {
        button: MouseButtonRequest,
    },
    Wheel {
        horizontal_delta: i32,
        vertical_delta: i32,
    },
}

impl MouseInputRequest {
    fn into_events(self) -> anyhow::Result<Vec<InputEvent>> {
        match self {
            Self::Move { display_id, x, y } => Ok(vec![InputEvent::MouseMove {
                display_id: display_id.map(DisplayId::new).transpose()?,
                normalized_x: normalize_unit_coordinate(x)?,
                normalized_y: normalize_unit_coordinate(y)?,
            }]),
            Self::ButtonDown { button, x, y } => Ok(vec![
                InputEvent::MouseMove {
                    display_id: None,
                    normalized_x: normalize_unit_coordinate(x)?,
                    normalized_y: normalize_unit_coordinate(y)?,
                },
                InputEvent::MouseButtonDown {
                    button: button.into(),
                },
            ]),
            Self::ButtonUp { button } => Ok(vec![InputEvent::MouseButtonUp {
                button: button.into(),
            }]),
            Self::Wheel {
                horizontal_delta,
                vertical_delta,
            } => {
                let mut events = Vec::with_capacity(2);
                if horizontal_delta != 0 {
                    events.push(InputEvent::MouseWheel {
                        axis: WheelAxis::Horizontal,
                        delta: horizontal_delta,
                    });
                }
                if vertical_delta != 0 {
                    events.push(InputEvent::MouseWheel {
                        axis: WheelAxis::Vertical,
                        delta: vertical_delta,
                    });
                }
                Ok(events)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum KeyboardInputRequest {
    KeyDown { key: KeyCode },
    KeyUp { key: KeyCode },
}

impl From<KeyboardInputRequest> for InputEvent {
    fn from(value: KeyboardInputRequest) -> Self {
        match value {
            KeyboardInputRequest::KeyDown { key } => Self::KeyDown { key },
            KeyboardInputRequest::KeyUp { key } => Self::KeyUp { key },
        }
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn connect_remote(
    app: AppHandle,
    control: State<'_, StreamControl>,
    request: ConnectRequest,
) -> Result<(), String> {
    let mut active = control
        .active
        .lock()
        .map_err(|_| "stream control lock is unavailable".to_owned())?;
    if let Some(previous) = active.take() {
        let _result = previous.cancellation.send(());
    }
    let (cancel_sender, cancel_receiver) = oneshot::channel();
    let (input_sender, input_receiver) = mpsc::channel(INPUT_QUEUE_CAPACITY);
    *active = Some(ActiveStream {
        cancellation: cancel_sender,
        input: input_sender,
    });
    drop(active);

    tauri::async_runtime::spawn(async move {
        emit_status(&app, "connecting", "Connecting to relay");
        let result = receive_video(app.clone(), request, cancel_receiver, input_receiver).await;
        match result {
            Ok(()) => emit_status(&app, "disconnected", "Remote session ended"),
            Err(error) => emit_status(&app, "error", &error.to_string()),
        }
    });
    Ok(())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn disconnect_remote(app: AppHandle, control: State<'_, StreamControl>) -> Result<(), String> {
    let mut active = control
        .active
        .lock()
        .map_err(|_| "stream control lock is unavailable".to_owned())?;
    if let Some(stream) = active.take() {
        let _result = stream.cancellation.send(());
    }
    emit_status(&app, "disconnected", "Disconnected by user");
    Ok(())
}

#[tauri::command]
async fn send_mouse_input(
    control: State<'_, StreamControl>,
    request: MouseInputRequest,
) -> Result<(), String> {
    let sender = control
        .active
        .lock()
        .map_err(|_| "stream control lock is unavailable".to_owned())?
        .as_ref()
        .map(|stream| stream.input.clone())
        .ok_or_else(|| "remote session is not connected".to_owned())?;
    for event in request.into_events().map_err(|error| error.to_string())? {
        sender
            .send(event)
            .await
            .map_err(|_| "remote session input channel is closed".to_owned())?;
    }
    Ok(())
}

#[tauri::command]
async fn send_keyboard_input(
    control: State<'_, StreamControl>,
    request: KeyboardInputRequest,
) -> Result<(), String> {
    let sender = control
        .active
        .lock()
        .map_err(|_| "stream control lock is unavailable".to_owned())?
        .as_ref()
        .map(|stream| stream.input.clone())
        .ok_or_else(|| "remote session is not connected".to_owned())?;
    sender
        .send(request.into())
        .await
        .map_err(|_| "remote session input channel is closed".to_owned())
}

async fn receive_video(
    app: AppHandle,
    request: ConnectRequest,
    mut cancellation: oneshot::Receiver<()>,
    mut input_receiver: mpsc::Receiver<InputEvent>,
) -> anyhow::Result<()> {
    let relay_address: SocketAddr = request
        .relay_address
        .parse()
        .context("parse relay address")?;
    let session_id: SessionId = request.session_id.parse().context("parse session ID")?;
    let token = parse_token(&request.token_hex)?;
    let end_to_end_key = parse_key(&request.end_to_end_key_hex)?;
    let video_cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let input_cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::ControllerToAgent,
    );
    let endpoint = client_endpoint(Path::new(&request.ca_certificate_path))?;
    let connection = endpoint
        .connect(relay_address, &request.server_name)
        .context("create relay connection")?
        .await
        .context("connect to relay")?;
    let (send, receive) = connection.open_bi().await.context("open relay stream")?;
    let mut transport = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
    let hello = RelayClientMessage::ClientHello(remotex_protocol::ClientHello::new(
        session_id,
        Role::Controller,
        token,
    ));
    transport.send(Bytes::from(encode_wire(&hello)?)).await?;
    wait_for_peer(&app, &mut transport).await?;
    emit_status(&app, "connected", "Remote peer ready");

    let mut input_sequence = 0_u64;
    loop {
        let relay_message = tokio::select! {
            _ = &mut cancellation => break,
            result = transport.receive() => decode_wire::<RelayServerMessage>(&result?)?,
            event = input_receiver.recv() => {
                let Some(event) = event else { break; };
                send_input_event(
                    &mut transport,
                    session_id,
                    input_sequence,
                    &input_cipher,
                    event,
                ).await?;
                input_sequence = input_sequence
                    .checked_add(1)
                    .context("input sequence space exhausted")?;
                continue;
            }
        };
        let bytes = match relay_message {
            RelayServerMessage::Payload(payload) => payload,
            RelayServerMessage::Heartbeat { nonce } => {
                let acknowledgement = RelayClientMessage::HeartbeatAck { nonce };
                transport
                    .send(Bytes::from(encode_wire(&acknowledgement)?))
                    .await?;
                continue;
            }
            RelayServerMessage::HeartbeatAck { .. }
            | RelayServerMessage::WaitingForPeer { .. }
            | RelayServerMessage::PeerReady => continue,
            RelayServerMessage::SessionClosed { reason } => {
                anyhow::bail!("relay session closed: {reason:?}");
            }
            RelayServerMessage::ProtocolError { code, message } => {
                anyhow::bail!("relay protocol error {code:?}: {message}");
            }
        };
        emit_video_payload(&app, &bytes, session_id, &video_cipher)?;
    }
    let _result = transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::Close)?))
        .await;
    endpoint.close(0_u32.into(), b"controller disconnected");
    Ok(())
}

fn emit_video_payload(
    app: &AppHandle,
    bytes: &[u8],
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
) -> anyhow::Result<()> {
    if bytes.len() < 8 {
        anyhow::bail!("encrypted video frame is missing its sequence number");
    }
    let sequence = u64::from_be_bytes(
        bytes[..8]
            .try_into()
            .context("read encrypted video sequence")?,
    );
    let plaintext = cipher
        .open(sequence, &bytes[8..])
        .context("authenticate and decrypt video envelope")?;
    let envelope: MessageEnvelope = decode_wire(&plaintext).context("decode protocol envelope")?;
    envelope.validate()?;
    if envelope.session_id != session_id {
        anyhow::bail!("received a frame for a different session");
    }
    if envelope.sequence != sequence {
        anyhow::bail!("encrypted frame sequence does not match its envelope");
    }
    let Message::Video(frame) = envelope.message else {
        return Ok(());
    };
    let mime_type = match frame.codec {
        VideoCodec::Jpeg => "image/jpeg",
        VideoCodec::WebP => "image/webp",
    };
    app.emit(
        "video-frame",
        VideoFrameEvent {
            sequence: envelope.sequence,
            width: frame.width,
            height: frame.height,
            source_timestamp_ms: frame.source_timestamp_ms,
            mime_type,
            data: STANDARD.encode(frame.payload),
        },
    )?;
    Ok(())
}

async fn send_input_event(
    transport: &mut QuicFrameConnection,
    session_id: SessionId,
    sequence: u64,
    cipher: &XChaChaSessionCipher,
    event: InputEvent,
) -> anyhow::Result<()> {
    let envelope = MessageEnvelope::new(session_id, sequence, now_ms()?, Message::Input(event));
    let plaintext = encode_wire(&envelope)?;
    let ciphertext = cipher
        .seal(sequence, &plaintext)
        .context("encrypt input envelope")?;
    let mut payload = Vec::with_capacity(8 + ciphertext.len());
    payload.extend_from_slice(&sequence.to_be_bytes());
    payload.extend_from_slice(&ciphertext);
    let relay_message = RelayClientMessage::Payload(payload);
    transport
        .send(Bytes::from(encode_wire(&relay_message)?))
        .await?;
    Ok(())
}

async fn wait_for_peer(app: &AppHandle, transport: &mut QuicFrameConnection) -> anyhow::Result<()> {
    loop {
        let message: RelayServerMessage = decode_wire(&transport.receive().await?)?;
        match message {
            RelayServerMessage::WaitingForPeer { role } => {
                emit_status(app, "connecting", &format!("Waiting for {role:?}"));
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

fn parse_token(value: &str) -> anyhow::Result<SessionToken> {
    let decoded = hex::decode(value).context("decode 64-character token hex")?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("session token must contain exactly 32 bytes"))?;
    Ok(SessionToken::from_bytes(bytes))
}

fn parse_key(value: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = hex::decode(value).context("decode 64-character end-to-end key hex")?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("end-to-end key must contain exactly 32 bytes"))
}

fn now_ms() -> anyhow::Result<u64> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    u64::try_from(duration.as_millis()).context("system timestamp is out of range")
}

fn emit_status(app: &AppHandle, state: &'static str, message: &str) {
    let _result = app.emit(
        "connection-status",
        StatusEvent {
            state,
            message: message.to_owned(),
        },
    );
}

fn main() {
    tauri::Builder::default()
        .manage(StreamControl::default())
        .invoke_handler(tauri::generate_handler![
            connect_remote,
            disconnect_remote,
            send_mouse_input,
            send_keyboard_input
        ])
        .run(tauri::generate_context!())
        .expect("run RemoteX desktop application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_down_moves_before_pressing() {
        let events = MouseInputRequest::ButtonDown {
            button: MouseButtonRequest::Right,
            x: 0.25,
            y: 0.75,
        }
        .into_events()
        .expect("translate pointer down");

        assert_eq!(
            events,
            vec![
                InputEvent::MouseMove {
                    display_id: None,
                    normalized_x: 16_384,
                    normalized_y: 49_151,
                },
                InputEvent::MouseButtonDown {
                    button: MouseButton::Right,
                },
            ]
        );
    }

    #[test]
    fn wheel_axes_are_translated_independently() {
        let events = MouseInputRequest::Wheel {
            horizontal_delta: -30,
            vertical_delta: 120,
        }
        .into_events()
        .expect("translate wheel");

        assert_eq!(
            events,
            vec![
                InputEvent::MouseWheel {
                    axis: WheelAxis::Horizontal,
                    delta: -30,
                },
                InputEvent::MouseWheel {
                    axis: WheelAxis::Vertical,
                    delta: 120,
                },
            ]
        );
    }

    #[test]
    fn keyboard_requests_preserve_physical_keys_and_state() {
        assert_eq!(
            InputEvent::from(KeyboardInputRequest::KeyDown {
                key: KeyCode::ControlLeft,
            }),
            InputEvent::KeyDown {
                key: KeyCode::ControlLeft,
            }
        );
        assert_eq!(
            InputEvent::from(KeyboardInputRequest::KeyUp { key: KeyCode::KeyV }),
            InputEvent::KeyUp { key: KeyCode::KeyV }
        );
    }
}
