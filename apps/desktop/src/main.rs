//! `RemoteX` M7 Tauri Controller for remote desktop, clipboard, and files.

use anyhow::Context;
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use quinn::{ClientConfig, Endpoint};
use remotex_clipboard::{ClipboardError, PermissionedClipboard, WindowsClipboardBackend};
use remotex_crypto::{SessionCipher, SessionDirection, XChaChaSessionCipher};
use remotex_file_transfer::{
    FileTransferError, IncomingTransfer, OutgoingTransfer, TransferRegistry,
};
use remotex_input::normalize_unit_coordinate;
use remotex_protocol::{
    ClipboardOrigin, ConnectionType, ConnectivityCandidate, ConnectivityCandidateKind,
    ControlMessage, CreateSessionRequest, DeviceId, DisplayId, EncodedVideoFrame, FileEntry,
    FileEntryKind, FileTransferDirection, FileTransferMessage, InputEvent, KeyCode,
    MAX_FILE_CHUNK_SIZE, Message, MessageEnvelope, MouseButton, RelayClientMessage,
    RelayServerMessage, Role, SessionCredentials, SessionId, SessionPermissions, SessionToken,
    TransferId, VideoCodec, VideoFeedback, WheelAxis, decode_wire, encode_wire,
};
use remotex_transport::{
    Connection, DEFAULT_DIRECT_ATTEMPT_TIMEOUT, DEFAULT_MAX_FRAME_SIZE, QuicFrameConnection,
    connect_direct_candidates,
};
use remotex_video::{StreamDecoder, VideoDecoder};
use rustls::RootCertStore;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;
use tracing::warn;

const INPUT_QUEUE_CAPACITY: usize = 128;
const FILE_COMMAND_QUEUE_CAPACITY: usize = 8;
const FILE_RESPONSE_QUEUE_CAPACITY: usize = 2;

#[derive(Default)]
struct StreamControl {
    active: std::sync::Mutex<Option<ActiveStream>>,
}

struct ActiveStream {
    cancellation: oneshot::Sender<()>,
    input: mpsc::Sender<InputEvent>,
    files: mpsc::Sender<ControllerFileCommand>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectRequest {
    #[serde(default)]
    control_server_url: String,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    controller_name: String,
    #[serde(default)]
    unattended_secret: String,
    relay_address: String,
    server_name: String,
    ca_certificate_path: String,
    session_id: String,
    token_hex: String,
    end_to_end_key_hex: String,
    clipboard_enabled: bool,
    file_upload_enabled: bool,
    file_download_enabled: bool,
}

struct ResolvedSession {
    relay_address: String,
    server_name: String,
    session_id: String,
    token_hex: String,
    end_to_end_key_hex: String,
    permissions: SessionPermissions,
    peer_candidates: Vec<ConnectivityCandidate>,
}

#[derive(Debug)]
enum ControllerFileCommand {
    List {
        path: String,
    },
    CreateDirectory {
        path: String,
    },
    Upload {
        transfer_id: Option<TransferId>,
        local_path: PathBuf,
        destination_path: String,
    },
    Download {
        transfer_id: Option<TransferId>,
        source_path: String,
        local_path: PathBuf,
    },
    Cancel {
        transfer_id: TransferId,
    },
}

#[derive(Debug)]
struct ControllerFileService {
    upload_permission: bool,
    download_permission: bool,
    next_request_id: u64,
    uploads: TransferRegistry<OutgoingTransfer>,
    downloads: TransferRegistry<IncomingTransfer>,
    pending_downloads: HashMap<TransferId, PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteFileEntryEvent {
    name: String,
    path: String,
    entry_type: &'static str,
    size: u64,
    modified_ms: Option<u64>,
}

impl From<FileEntry> for RemoteFileEntryEvent {
    fn from(entry: FileEntry) -> Self {
        Self {
            name: entry.name,
            path: entry.path,
            entry_type: match entry.kind {
                FileEntryKind::File => "file",
                FileEntryKind::Directory => "directory",
            },
            size: entry.size,
            modified_ms: entry.modified_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum FileEvent {
    Directory {
        path: String,
        entries: Vec<RemoteFileEntryEvent>,
    },
    DirectoryCreated {
        path: String,
    },
    Progress {
        transfer_id: String,
        direction: &'static str,
        transferred: u64,
        total: u64,
        state: &'static str,
    },
    Error {
        transfer_id: Option<String>,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoFrameEvent {
    sequence: u64,
    frame_id: u64,
    width: u32,
    height: u32,
    frames_per_second: u32,
    bitrate_bps: u32,
    source_timestamp_ms: u64,
    capture_latency_ms: u32,
    encode_latency_ms: u32,
    decode_latency_ms: u32,
    end_to_end_latency_ms: u64,
    codec: &'static str,
    key_frame: bool,
    mime_type: &'static str,
    data: String,
}

#[derive(Default)]
struct AgentMessageOutcome {
    file_message: Option<FileTransferMessage>,
    video_feedback: Option<VideoFeedback>,
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
    let (file_sender, file_receiver) = mpsc::channel(FILE_COMMAND_QUEUE_CAPACITY);
    *active = Some(ActiveStream {
        cancellation: cancel_sender,
        input: input_sender,
        files: file_sender,
    });
    drop(active);

    tauri::async_runtime::spawn(async move {
        emit_status(&app, "connecting", "Connecting to relay");
        let result = run_remote_session(
            app.clone(),
            request,
            cancel_receiver,
            input_receiver,
            file_receiver,
        )
        .await;
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

fn active_file_sender(
    control: &State<'_, StreamControl>,
) -> Result<mpsc::Sender<ControllerFileCommand>, String> {
    control
        .active
        .lock()
        .map_err(|_| "stream control lock is unavailable".to_owned())?
        .as_ref()
        .map(|stream| stream.files.clone())
        .ok_or_else(|| "remote session is not connected".to_owned())
}

#[tauri::command]
async fn list_remote_files(control: State<'_, StreamControl>, path: String) -> Result<(), String> {
    active_file_sender(&control)?
        .send(ControllerFileCommand::List { path })
        .await
        .map_err(|_| "remote file service is closed".to_owned())
}

#[tauri::command]
async fn create_remote_directory(
    control: State<'_, StreamControl>,
    path: String,
) -> Result<(), String> {
    active_file_sender(&control)?
        .send(ControllerFileCommand::CreateDirectory { path })
        .await
        .map_err(|_| "remote file service is closed".to_owned())
}

#[tauri::command]
async fn upload_remote_file(
    control: State<'_, StreamControl>,
    local_path: String,
    destination_path: String,
) -> Result<(), String> {
    active_file_sender(&control)?
        .send(ControllerFileCommand::Upload {
            transfer_id: None,
            local_path: PathBuf::from(local_path),
            destination_path,
        })
        .await
        .map_err(|_| "remote file service is closed".to_owned())
}

#[tauri::command]
async fn download_remote_file(
    control: State<'_, StreamControl>,
    source_path: String,
    local_path: String,
) -> Result<(), String> {
    active_file_sender(&control)?
        .send(ControllerFileCommand::Download {
            transfer_id: None,
            source_path,
            local_path: PathBuf::from(local_path),
        })
        .await
        .map_err(|_| "remote file service is closed".to_owned())
}

#[tauri::command]
async fn resume_file_upload(
    control: State<'_, StreamControl>,
    transfer_id: String,
    local_path: String,
    destination_path: String,
) -> Result<(), String> {
    let transfer_id = transfer_id
        .parse()
        .map_err(|_| "transfer ID is invalid".to_owned())?;
    active_file_sender(&control)?
        .send(ControllerFileCommand::Upload {
            transfer_id: Some(transfer_id),
            local_path: PathBuf::from(local_path),
            destination_path,
        })
        .await
        .map_err(|_| "remote file service is closed".to_owned())
}

#[tauri::command]
async fn resume_file_download(
    control: State<'_, StreamControl>,
    transfer_id: String,
    source_path: String,
    local_path: String,
) -> Result<(), String> {
    let transfer_id = transfer_id
        .parse()
        .map_err(|_| "transfer ID is invalid".to_owned())?;
    active_file_sender(&control)?
        .send(ControllerFileCommand::Download {
            transfer_id: Some(transfer_id),
            source_path,
            local_path: PathBuf::from(local_path),
        })
        .await
        .map_err(|_| "remote file service is closed".to_owned())
}

#[tauri::command]
async fn cancel_file_transfer(
    control: State<'_, StreamControl>,
    transfer_id: String,
) -> Result<(), String> {
    let transfer_id = transfer_id
        .parse()
        .map_err(|_| "transfer ID is invalid".to_owned())?;
    active_file_sender(&control)?
        .send(ControllerFileCommand::Cancel { transfer_id })
        .await
        .map_err(|_| "remote file service is closed".to_owned())
}

#[allow(clippy::too_many_lines)]
async fn run_remote_session(
    app: AppHandle,
    request: ConnectRequest,
    mut cancellation: oneshot::Receiver<()>,
    mut input_receiver: mpsc::Receiver<InputEvent>,
    file_commands: mpsc::Receiver<ControllerFileCommand>,
) -> anyhow::Result<()> {
    if !request.control_server_url.trim().is_empty() {
        emit_status(
            &app,
            "authorizing",
            "Waiting for authorization on the remote device",
        );
    }
    let resolved = resolve_session(&request).await?;
    let relay_address: SocketAddr = resolved
        .relay_address
        .parse()
        .context("parse relay address")?;
    let session_id: SessionId = resolved.session_id.parse().context("parse session ID")?;
    let token = parse_token(&resolved.token_hex)?;
    let end_to_end_key = parse_key(&resolved.end_to_end_key_hex)?;
    let inbound_cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let outbound_cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::ControllerToAgent,
    );
    let endpoint = client_endpoint(Path::new(&request.ca_certificate_path))?;
    let connection = endpoint
        .connect(relay_address, &resolved.server_name)
        .context("create relay connection")?
        .await
        .context("connect to relay")?;
    let (send, receive) = connection.open_bi().await.context("open relay stream")?;
    let mut relay_transport = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
    let hello = RelayClientMessage::ClientHello(remotex_protocol::ClientHello::new(
        session_id,
        Role::Controller,
        token,
    ));
    relay_transport
        .send(Bytes::from(encode_wire(&hello)?))
        .await?;
    wait_for_peer(&app, &mut relay_transport).await?;
    let (mut transport, connection_type): (Box<dyn Connection>, ConnectionType) =
        if resolved.peer_candidates.is_empty() {
            (Box::new(relay_transport), ConnectionType::Relay)
        } else {
            emit_status(
                &app,
                "connecting",
                "Relay ready · trying authenticated direct path",
            );
            match connect_direct_candidates(
                &endpoint,
                &resolved.peer_candidates,
                session_id,
                &end_to_end_key,
                DEFAULT_DIRECT_ATTEMPT_TIMEOUT,
            )
            .await
            {
                Ok((connection, candidate_kind)) => {
                    let connection_type = match candidate_kind {
                        ConnectivityCandidateKind::Lan => ConnectionType::Lan,
                        ConnectivityCandidateKind::ServerReflexive => ConnectionType::Direct,
                    };
                    (Box::new(connection), connection_type)
                }
                Err(error) => {
                    warn!(event = "direct_connection_failed", %session_id, %error);
                    (Box::new(relay_transport), ConnectionType::Relay)
                }
            }
        };
    emit_status(
        &app,
        "connected",
        match connection_type {
            ConnectionType::Lan => "Remote peer ready · Connection: LAN",
            ConnectionType::Direct => "Remote peer ready · Connection: Direct",
            ConnectionType::Relay => "Remote peer ready · Connection: Relay",
        },
    );

    let mut clipboard = PermissionedClipboard::new(
        WindowsClipboardBackend,
        ClipboardOrigin::Controller,
        resolved.permissions.clipboard,
    );
    let mut outbound_sequence = 0_u64;
    let mut expected_inbound_sequence = 0_u64;
    let mut video_decoder = StreamDecoder::new()?;
    let mut last_video_feedback_ms = 0_u64;
    send_controller_message(
        transport.as_mut(),
        session_id,
        &outbound_cipher,
        &mut outbound_sequence,
        Message::Control(ControlMessage::VideoCapabilities {
            codecs: vec![VideoCodec::H264, VideoCodec::Jpeg, VideoCodec::WebP],
        }),
    )
    .await?;
    let mut clipboard_interval = tokio::time::interval(std::time::Duration::from_millis(500));
    clipboard_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let file_service = ControllerFileService {
        upload_permission: resolved.permissions.file_upload,
        download_permission: resolved.permissions.file_download,
        next_request_id: 1,
        uploads: TransferRegistry::default(),
        downloads: TransferRegistry::default(),
        pending_downloads: HashMap::new(),
    };
    let (remote_file_sender, remote_file_receiver) = mpsc::channel(FILE_COMMAND_QUEUE_CAPACITY);
    let (file_response_sender, mut file_response_receiver) =
        mpsc::channel(FILE_RESPONSE_QUEUE_CAPACITY);
    let file_worker = tokio::spawn(file_service.run(
        app.clone(),
        file_commands,
        remote_file_receiver,
        file_response_sender,
    ));
    loop {
        tokio::select! {
            _ = &mut cancellation => break,
            result = transport.receive() => {
                let relay_message = decode_wire::<RelayServerMessage>(&result?)?;
                let outcome = handle_relay_message(
                    &app,
                    transport.as_mut(),
                    relay_message,
                    session_id,
                    &inbound_cipher,
                    &mut expected_inbound_sequence,
                    &mut clipboard,
                    &mut video_decoder,
                ).await?;
                if let Some(message) = outcome.file_message {
                    remote_file_sender
                        .try_send(message)
                        .map_err(|_| anyhow::anyhow!("file command queue is full or closed"))?;
                }
                if let Some(feedback) = outcome.video_feedback {
                    let current_ms = now_ms()?;
                    if current_ms.saturating_sub(last_video_feedback_ms) >= 1_000 {
                        send_controller_message(
                            transport.as_mut(),
                            session_id,
                            &outbound_cipher,
                            &mut outbound_sequence,
                            Message::Control(ControlMessage::VideoFeedback(feedback)),
                        ).await?;
                        last_video_feedback_ms = current_ms;
                    }
                }
            }
            event = input_receiver.recv(), if resolved.permissions.control_input => {
                let Some(event) = event else { break; };
                send_controller_message(
                    transport.as_mut(),
                    session_id,
                    &outbound_cipher,
                    &mut outbound_sequence,
                    Message::Input(event),
                ).await?;
            }
            _ = clipboard_interval.tick(), if clipboard.is_enabled() => {
                match clipboard.poll() {
                    Ok(Some(message)) => send_controller_message(
                        transport.as_mut(),
                        session_id,
                        &outbound_cipher,
                        &mut outbound_sequence,
                        Message::Clipboard(message),
                    ).await?,
                    Ok(None) => {}
                    Err(error) => warn!(event = "clipboard_poll_failed", %error),
                }
            }
            response = file_response_receiver.recv() => {
                let Some(response) = response else {
                    anyhow::bail!("file service stopped unexpectedly");
                };
                send_controller_message(
                    transport.as_mut(),
                    session_id,
                    &outbound_cipher,
                    &mut outbound_sequence,
                    Message::FileTransfer(response),
                ).await?;
            }
        }
    }
    file_worker.abort();
    let _result = transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::Close)?))
        .await;
    endpoint.close(0_u32.into(), b"controller disconnected");
    Ok(())
}

async fn resolve_session(request: &ConnectRequest) -> anyhow::Result<ResolvedSession> {
    let requested_permissions = SessionPermissions {
        view_desktop: true,
        control_input: true,
        clipboard: request.clipboard_enabled,
        file_upload: request.file_upload_enabled,
        file_download: request.file_download_enabled,
    };
    if request.control_server_url.trim().is_empty() {
        return Ok(ResolvedSession {
            relay_address: request.relay_address.clone(),
            server_name: request.server_name.clone(),
            session_id: request.session_id.clone(),
            token_hex: request.token_hex.clone(),
            end_to_end_key_hex: request.end_to_end_key_hex.clone(),
            permissions: requested_permissions,
            peer_candidates: Vec::new(),
        });
    }

    let device_id = DeviceId::new(request.device_id.trim()).context("validate device ID")?;
    let controller_name = request.controller_name.trim();
    if controller_name.is_empty() {
        anyhow::bail!("controller name is required when using the control server");
    }
    let url = format!(
        "{}/api/sessions",
        request.control_server_url.trim().trim_end_matches('/')
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&CreateSessionRequest {
            device_id,
            controller_name: controller_name.to_owned(),
            requested_permissions,
            unattended_secret: (!request.unattended_secret.is_empty())
                .then(|| request.unattended_secret.clone()),
        })
        .send()
        .await
        .context("request a managed session")?
        .error_for_status()
        .context("control server rejected the session request")?
        .json::<SessionCredentials>()
        .await
        .context("decode managed session credentials")?;

    Ok(ResolvedSession {
        relay_address: response.relay_address,
        server_name: response.relay_server_name,
        session_id: response.session_id.to_string(),
        token_hex: response.role_token_hex,
        end_to_end_key_hex: response.end_to_end_key_hex,
        permissions: response.permissions,
        peer_candidates: response.peer_candidates,
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_relay_message(
    app: &AppHandle,
    transport: &mut dyn Connection,
    message: RelayServerMessage,
    session_id: SessionId,
    inbound_cipher: &XChaChaSessionCipher,
    expected_sequence: &mut u64,
    clipboard: &mut PermissionedClipboard<WindowsClipboardBackend>,
    video_decoder: &mut StreamDecoder,
) -> anyhow::Result<AgentMessageOutcome> {
    match message {
        RelayServerMessage::Payload(payload) => handle_agent_payload(
            app,
            &payload,
            session_id,
            inbound_cipher,
            expected_sequence,
            clipboard,
            video_decoder,
        ),
        RelayServerMessage::Heartbeat { nonce } => {
            let acknowledgement = RelayClientMessage::HeartbeatAck { nonce };
            transport
                .send(Bytes::from(encode_wire(&acknowledgement)?))
                .await?;
            Ok(AgentMessageOutcome::default())
        }
        RelayServerMessage::HeartbeatAck { .. }
        | RelayServerMessage::WaitingForPeer { .. }
        | RelayServerMessage::PeerReady => Ok(AgentMessageOutcome::default()),
        RelayServerMessage::SessionClosed { reason } => {
            anyhow::bail!("relay session closed: {reason:?}");
        }
        RelayServerMessage::ProtocolError { code, message } => {
            anyhow::bail!("relay protocol error {code:?}: {message}");
        }
    }
}

fn handle_agent_payload(
    app: &AppHandle,
    bytes: &[u8],
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    expected_sequence: &mut u64,
    clipboard: &mut PermissionedClipboard<WindowsClipboardBackend>,
    video_decoder: &mut StreamDecoder,
) -> anyhow::Result<AgentMessageOutcome> {
    if bytes.len() > MAX_FILE_CHUNK_SIZE as usize + 64 * 1024 {
        anyhow::bail!("Agent data payload exceeds the M7 limit");
    }
    if bytes.len() < 8 {
        anyhow::bail!("encrypted Agent payload is missing its sequence number");
    }
    let sequence = u64::from_be_bytes(
        bytes[..8]
            .try_into()
            .context("read encrypted Agent sequence")?,
    );
    if sequence != *expected_sequence {
        anyhow::bail!(
            "unexpected Agent sequence {sequence}; expected {}",
            *expected_sequence
        );
    }
    let plaintext = cipher
        .open(sequence, &bytes[8..])
        .context("authenticate and decrypt Agent envelope")?;
    let envelope: MessageEnvelope = decode_wire(&plaintext).context("decode protocol envelope")?;
    envelope.validate()?;
    if envelope.session_id != session_id {
        anyhow::bail!("received an Agent message for a different session");
    }
    if envelope.sequence != sequence {
        anyhow::bail!("encrypted Agent sequence does not match its envelope");
    }
    let mut outcome = AgentMessageOutcome::default();
    match envelope.message {
        Message::Video(frame) => {
            outcome.video_feedback = Some(emit_video_frame(app, sequence, &frame, video_decoder)?);
        }
        Message::Clipboard(message) => match clipboard.apply(message) {
            Ok(_) => {}
            Err(ClipboardError::PermissionDenied) => {
                warn!(event = "clipboard_permission_denied", %session_id);
            }
            Err(error) => return Err(error.into()),
        },
        Message::FileTransfer(message) => outcome.file_message = Some(message),
        _ => anyhow::bail!("Agent sent a message not allowed in its data direction"),
    }
    *expected_sequence = expected_sequence
        .checked_add(1)
        .context("Agent inbound sequence space exhausted")?;
    Ok(outcome)
}

fn emit_video_frame(
    app: &AppHandle,
    sequence: u64,
    frame: &EncodedVideoFrame,
    decoder: &mut StreamDecoder,
) -> anyhow::Result<VideoFeedback> {
    let decoded_frame = decoder.decode_frame(frame)?;
    let frame_budget_ms = 1_000 / frame.frames_per_second.max(1);
    let queue_percent = decoded_frame
        .decode_latency_ms
        .saturating_mul(100)
        .checked_div(frame_budget_ms)
        .unwrap_or(100)
        .min(100) as u8;
    let codec = match frame.codec {
        VideoCodec::H264 => "H.264",
        VideoCodec::Jpeg => "JPEG",
        VideoCodec::WebP => "WebP",
    };
    let end_to_end_latency_ms = now_ms()?.saturating_sub(frame.source_timestamp_ms);
    app.emit(
        "video-frame",
        VideoFrameEvent {
            sequence,
            frame_id: frame.frame_id,
            width: frame.width,
            height: frame.height,
            frames_per_second: frame.frames_per_second,
            bitrate_bps: frame.bitrate_bps,
            source_timestamp_ms: frame.source_timestamp_ms,
            capture_latency_ms: frame.capture_latency_ms,
            encode_latency_ms: frame.encode_latency_ms,
            decode_latency_ms: decoded_frame.decode_latency_ms,
            end_to_end_latency_ms,
            codec,
            key_frame: frame.key_frame,
            mime_type: "application/x-remotex-rgba",
            data: STANDARD.encode(decoded_frame.rgba),
        },
    )?;
    Ok(VideoFeedback {
        rtt_ms: 0,
        packet_loss_per_mille: 0,
        send_queue_percent: queue_percent,
        decoder_latency_ms: decoded_frame.decode_latency_ms,
        render_latency_ms: 0,
    })
}

impl ControllerFileService {
    async fn run(
        mut self,
        app: AppHandle,
        mut local_commands: mpsc::Receiver<ControllerFileCommand>,
        mut remote_commands: mpsc::Receiver<FileTransferMessage>,
        responses: mpsc::Sender<FileTransferMessage>,
    ) {
        loop {
            let response = tokio::select! {
                command = local_commands.recv() => {
                    let Some(command) = command else { break; };
                    self.handle_local(&app, command).await
                }
                message = remote_commands.recv() => {
                    let Some(message) = message else { break; };
                    self.handle_remote(&app, message).await
                }
            };
            if let Some(response) = response
                && responses.send(response).await.is_err()
            {
                break;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_local(
        &mut self,
        app: &AppHandle,
        command: ControllerFileCommand,
    ) -> Option<FileTransferMessage> {
        match command {
            ControllerFileCommand::List { path } => {
                if !self.upload_permission && !self.download_permission {
                    emit_file_error(app, None, "file browsing is disabled");
                    return None;
                }
                let request_id = self.take_request_id(app)?;
                Some(FileTransferMessage::ListDirectoryRequest { request_id, path })
            }
            ControllerFileCommand::CreateDirectory { path } => {
                if !self.upload_permission {
                    emit_file_error(app, None, "file upload permission is disabled");
                    return None;
                }
                let request_id = self.take_request_id(app)?;
                Some(FileTransferMessage::CreateDirectoryRequest { request_id, path })
            }
            ControllerFileCommand::Upload {
                transfer_id,
                local_path,
                destination_path,
            } => {
                if !self.upload_permission {
                    emit_file_error(app, None, "file upload permission is disabled");
                    return None;
                }
                let transfer_id = transfer_id.unwrap_or_default();
                match OutgoingTransfer::open(local_path, remotex_protocol::DEFAULT_FILE_CHUNK_SIZE)
                    .await
                {
                    Ok(transfer) => {
                        let filename = match transfer.filename() {
                            Ok(filename) => filename,
                            Err(error) => {
                                emit_local_file_error(app, Some(transfer_id), &error);
                                return None;
                            }
                        };
                        let message = FileTransferMessage::Start {
                            transfer_id,
                            direction: FileTransferDirection::Upload,
                            filename,
                            source_path: String::new(),
                            destination_path,
                            total_size: transfer.total_size(),
                            chunk_size: transfer.chunk_size(),
                            sha256: transfer.sha256(),
                        };
                        emit_file_progress(
                            app,
                            transfer_id,
                            "upload",
                            0,
                            transfer.total_size(),
                            "starting",
                        );
                        if let Err(error) = self.uploads.insert(transfer_id, transfer) {
                            emit_local_file_error(app, Some(transfer_id), &error);
                            return None;
                        }
                        Some(message)
                    }
                    Err(error) => {
                        emit_local_file_error(app, Some(transfer_id), &error);
                        None
                    }
                }
            }
            ControllerFileCommand::Download {
                transfer_id,
                source_path,
                local_path,
            } => {
                if !self.download_permission {
                    emit_file_error(app, None, "file download permission is disabled");
                    return None;
                }
                let transfer_id = transfer_id.unwrap_or_default();
                self.pending_downloads.insert(transfer_id, local_path);
                emit_file_progress(app, transfer_id, "download", 0, 0, "starting");
                Some(FileTransferMessage::DownloadRequest {
                    transfer_id,
                    source_path,
                })
            }
            ControllerFileCommand::Cancel { transfer_id } => {
                if self.uploads.contains(&transfer_id) {
                    let _ = self.uploads.remove(&transfer_id);
                }
                if self.pending_downloads.remove(&transfer_id).is_some() {
                    emit_file_progress(app, transfer_id, "download", 0, 0, "cancelled");
                }
                if self.downloads.contains(&transfer_id) {
                    match self.downloads.remove(&transfer_id) {
                        Ok(transfer) => {
                            if let Err(error) = transfer.cancel().await {
                                emit_local_file_error(app, Some(transfer_id), &error);
                            }
                        }
                        Err(error) => emit_local_file_error(app, Some(transfer_id), &error),
                    }
                }
                emit_file_progress(app, transfer_id, "transfer", 0, 0, "cancelled");
                Some(FileTransferMessage::Cancel { transfer_id })
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_remote(
        &mut self,
        app: &AppHandle,
        message: FileTransferMessage,
    ) -> Option<FileTransferMessage> {
        match message {
            FileTransferMessage::ListDirectoryResponse { path, entries, .. } => {
                let _ = app.emit(
                    "file-event",
                    FileEvent::Directory {
                        path,
                        entries: entries.into_iter().map(Into::into).collect(),
                    },
                );
                None
            }
            FileTransferMessage::CreateDirectoryResponse { path, .. } => {
                let _ = app.emit("file-event", FileEvent::DirectoryCreated { path });
                None
            }
            FileTransferMessage::Start {
                transfer_id,
                direction: FileTransferDirection::Download,
                total_size,
                chunk_size,
                sha256,
                ..
            } => {
                let Some(local_path) = self.pending_downloads.remove(&transfer_id) else {
                    emit_file_error(
                        app,
                        Some(transfer_id),
                        "download destination is unavailable",
                    );
                    return Some(FileTransferMessage::Cancel { transfer_id });
                };
                match IncomingTransfer::open(
                    transfer_id,
                    local_path,
                    total_size,
                    chunk_size,
                    sha256,
                )
                .await
                {
                    Ok(transfer) => {
                        let next_offset = transfer.next_offset();
                        emit_file_progress(
                            app,
                            transfer_id,
                            "download",
                            next_offset,
                            total_size,
                            "transferring",
                        );
                        if let Err(error) = self.downloads.insert(transfer_id, transfer) {
                            emit_local_file_error(app, Some(transfer_id), &error);
                            return Some(FileTransferMessage::Cancel { transfer_id });
                        }
                        Some(FileTransferMessage::Accept {
                            transfer_id,
                            next_offset,
                        })
                    }
                    Err(error) => {
                        emit_local_file_error(app, Some(transfer_id), &error);
                        Some(FileTransferMessage::Cancel { transfer_id })
                    }
                }
            }
            FileTransferMessage::Accept {
                transfer_id,
                next_offset,
            }
            | FileTransferMessage::ChunkAck {
                transfer_id,
                next_offset,
            }
            | FileTransferMessage::Resume {
                transfer_id,
                next_offset,
            } => self.upload_chunk(app, transfer_id, next_offset).await,
            FileTransferMessage::Chunk {
                transfer_id,
                offset,
                checksum,
                payload,
            } => {
                let result = match self.downloads.get_mut(&transfer_id) {
                    Ok(transfer) => {
                        let total = transfer.total_size();
                        match transfer.write_chunk(offset, checksum, &payload).await {
                            Ok(next_offset) => Ok((next_offset, total)),
                            Err(error) => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                match result {
                    Ok((next_offset, total)) => {
                        emit_file_progress(
                            app,
                            transfer_id,
                            "download",
                            next_offset,
                            total,
                            "transferring",
                        );
                        Some(FileTransferMessage::ChunkAck {
                            transfer_id,
                            next_offset,
                        })
                    }
                    Err(error) => {
                        emit_local_file_error(app, Some(transfer_id), &error);
                        Some(FileTransferMessage::Cancel { transfer_id })
                    }
                }
            }
            FileTransferMessage::Complete {
                transfer_id,
                total_size,
                sha256,
            } => {
                if self.downloads.contains(&transfer_id) {
                    let result = match self.downloads.remove(&transfer_id) {
                        Ok(transfer) => transfer.complete(total_size, sha256).await,
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(()) => emit_file_progress(
                            app,
                            transfer_id,
                            "download",
                            total_size,
                            total_size,
                            "completed",
                        ),
                        Err(error) => emit_local_file_error(app, Some(transfer_id), &error),
                    }
                } else if self.uploads.contains(&transfer_id) {
                    let _ = self.uploads.remove(&transfer_id);
                    emit_file_progress(
                        app,
                        transfer_id,
                        "upload",
                        total_size,
                        total_size,
                        "completed",
                    );
                }
                None
            }
            FileTransferMessage::Cancel { transfer_id } => {
                if self.uploads.contains(&transfer_id) {
                    let _ = self.uploads.remove(&transfer_id);
                }
                if self.downloads.contains(&transfer_id) {
                    let _ = self.downloads.remove(&transfer_id);
                }
                self.pending_downloads.remove(&transfer_id);
                emit_file_progress(app, transfer_id, "transfer", 0, 0, "cancelled");
                None
            }
            FileTransferMessage::Error {
                transfer_id,
                message,
                ..
            } => {
                if let Some(transfer_id) = transfer_id {
                    if self.uploads.contains(&transfer_id) {
                        let _ = self.uploads.remove(&transfer_id);
                    }
                    if self.downloads.contains(&transfer_id) {
                        let _ = self.downloads.remove(&transfer_id);
                    }
                    self.pending_downloads.remove(&transfer_id);
                }
                emit_file_error(app, transfer_id, &message);
                None
            }
            FileTransferMessage::Progress {
                transfer_id,
                next_offset,
            } => {
                emit_file_progress(app, transfer_id, "transfer", next_offset, 0, "transferring");
                None
            }
            FileTransferMessage::ListDirectoryRequest { .. }
            | FileTransferMessage::CreateDirectoryRequest { .. }
            | FileTransferMessage::DownloadRequest { .. }
            | FileTransferMessage::Start { .. } => None,
        }
    }

    async fn upload_chunk(
        &mut self,
        app: &AppHandle,
        transfer_id: TransferId,
        offset: u64,
    ) -> Option<FileTransferMessage> {
        let result = match self.uploads.get_mut(&transfer_id) {
            Ok(transfer) => {
                let total = transfer.total_size();
                let sha256 = transfer.sha256();
                match transfer.read_chunk(offset).await {
                    Ok(chunk) => Ok((chunk, total, sha256)),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        match result {
            Ok((Some((offset, checksum, payload)), total, _)) => {
                let next = offset + payload.len() as u64;
                emit_file_progress(app, transfer_id, "upload", next, total, "transferring");
                Some(FileTransferMessage::Chunk {
                    transfer_id,
                    offset,
                    checksum,
                    payload,
                })
            }
            Ok((None, total, sha256)) => Some(FileTransferMessage::Complete {
                transfer_id,
                total_size: total,
                sha256,
            }),
            Err(error) => {
                emit_local_file_error(app, Some(transfer_id), &error);
                Some(FileTransferMessage::Cancel { transfer_id })
            }
        }
    }

    fn take_request_id(&mut self, app: &AppHandle) -> Option<u64> {
        let request_id = self.next_request_id;
        if let Some(next) = request_id.checked_add(1) {
            self.next_request_id = next;
            Some(request_id)
        } else {
            emit_file_error(app, None, "file request sequence is exhausted");
            None
        }
    }
}

fn emit_file_progress(
    app: &AppHandle,
    transfer_id: TransferId,
    direction: &'static str,
    transferred: u64,
    total: u64,
    state: &'static str,
) {
    let _ = app.emit(
        "file-event",
        FileEvent::Progress {
            transfer_id: transfer_id.to_string(),
            direction,
            transferred,
            total,
            state,
        },
    );
}

fn emit_local_file_error(
    app: &AppHandle,
    transfer_id: Option<TransferId>,
    error: &FileTransferError,
) {
    emit_file_error(app, transfer_id, &error.to_string());
}

fn emit_file_error(app: &AppHandle, transfer_id: Option<TransferId>, message: &str) {
    let _ = app.emit(
        "file-event",
        FileEvent::Error {
            transfer_id: transfer_id.map(|id| id.to_string()),
            message: message.to_owned(),
        },
    );
}

async fn send_controller_message(
    transport: &mut dyn Connection,
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    sequence: &mut u64,
    message: Message,
) -> anyhow::Result<()> {
    let envelope = MessageEnvelope::new(session_id, *sequence, now_ms()?, message);
    let plaintext = encode_wire(&envelope)?;
    let ciphertext = cipher
        .seal(*sequence, &plaintext)
        .context("encrypt Controller envelope")?;
    let mut payload = Vec::with_capacity(8 + ciphertext.len());
    payload.extend_from_slice(&(*sequence).to_be_bytes());
    payload.extend_from_slice(&ciphertext);
    let relay_message = RelayClientMessage::Payload(payload);
    transport
        .send(Bytes::from(encode_wire(&relay_message)?))
        .await?;
    *sequence = sequence
        .checked_add(1)
        .context("Controller outbound sequence space exhausted")?;
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
            send_keyboard_input,
            list_remote_files,
            create_remote_directory,
            upload_remote_file,
            download_remote_file,
            resume_file_upload,
            resume_file_download,
            cancel_file_transfer
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
