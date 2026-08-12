//! `RemoteX` M7 Windows Agent with remote screen, input, clipboard, and files.

#[cfg(windows)]
mod windows_authorization;

#[cfg(windows)]
use anyhow::Context;
#[cfg(windows)]
use bytes::Bytes;
#[cfg(windows)]
use quinn::{ClientConfig, Endpoint, ServerConfig};
#[cfg(windows)]
use remotex_capture::{CaptureError, DxgiCapture, MonitorId, MonitorInfo, ScreenCapture};
#[cfg(windows)]
use remotex_clipboard::{ClipboardError, PermissionedClipboard, WindowsClipboardBackend};
#[cfg(windows)]
use remotex_crypto::{
    DeviceIdentity, Ed25519DeviceIdentity, SessionCipher, SessionDirection, XChaChaSessionCipher,
    device_auth_message, secrets_equal,
};
#[cfg(windows)]
use remotex_file_transfer::{
    AllowedRoot, FileTransferError, IncomingTransfer, OutgoingTransfer, RootedFileSystem,
    TransferRegistry,
};
#[cfg(windows)]
use remotex_input::{
    DisplayGeometry, InputController, InputError, PermissionedInputController, WindowsInputBackend,
};
#[cfg(windows)]
use remotex_protocol::{
    AuthorizationDecision, ClaimAgentSessionRequest, ClaimAgentSessionResponse, ClipboardOrigin,
    ConnectionType, ConnectivityCandidate, ConnectivityCandidateKind, ControlMessage,
    DeviceAuthProof, DeviceHeartbeatRequest, DeviceId, DevicePlatform, DeviceRegistrationRequest,
    DeviceRegistrationResponse, DisplayId, FileTransferDirection, FileTransferErrorCode,
    FileTransferMessage, IncomingSessionRequest, MAX_FILE_CHUNK_SIZE, Message, MessageEnvelope,
    RelayClientMessage, RelayServerMessage, ReportSessionEventRequest,
    ResolveSessionAuthorizationRequest, ResolveSessionAuthorizationResponse, Role,
    SessionAuditEventKind, SessionCredentials, SessionId, SessionPermissions, SessionToken,
    TransferId, decode_wire, encode_wire,
};
#[cfg(windows)]
use remotex_transport::{
    Connection, DEFAULT_DIRECT_ATTEMPT_TIMEOUT, DEFAULT_MAX_FRAME_SIZE, DirectPeerConnection,
    QuicFrameConnection, authenticate_direct_server,
};
#[cfg(windows)]
use remotex_video::SessionVideoEncoder;
#[cfg(windows)]
use rustls::RootCertStore;
#[cfg(windows)]
use std::{
    fs::{File, OpenOptions},
    io::BufReader,
    net::{IpAddr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
#[cfg(windows)]
use tokio::sync::{mpsc, oneshot};
#[cfg(windows)]
use tokio::time::MissedTickBehavior;
#[cfg(windows)]
use tracing::{info, warn};
#[cfg(windows)]
use tracing_subscriber::EnvFilter;

#[cfg(windows)]
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredIdentity {
    secret_key_hex: String,
}

#[cfg(windows)]
struct ManagedAgentConfig {
    control_url: String,
    identity_path: PathBuf,
    relay_certificate_path: String,
    device_name: String,
    frames_per_second: u32,
    local_permissions: SessionPermissions,
    unattended_access: bool,
    unattended_secret: Option<String>,
    file_roots: Vec<AllowedRoot>,
    direct: Option<DirectServerSettings>,
}

#[cfg(windows)]
#[derive(Clone)]
struct DirectServerSettings {
    bind: SocketAddr,
    certificate_path: PathBuf,
    private_key_path: PathBuf,
    candidates: Vec<ConnectivityCandidate>,
}

#[cfg(windows)]
#[allow(clippy::struct_excessive_bools)]
struct AgentConfig {
    relay_address: SocketAddr,
    server_name: String,
    certificate_path: String,
    session_id: SessionId,
    token: SessionToken,
    end_to_end_key: [u8; 32],
    frames_per_second: u32,
    view_permission: bool,
    input_permission: bool,
    clipboard_permission: bool,
    file_upload_permission: bool,
    file_download_permission: bool,
    file_roots: Vec<AllowedRoot>,
    direct: Option<DirectServerSettings>,
}

#[cfg(windows)]
const FILE_COMMAND_QUEUE_CAPACITY: usize = 8;

#[cfg(windows)]
const FILE_RESPONSE_QUEUE_CAPACITY: usize = 2;

#[cfg(windows)]
struct AgentFileService {
    filesystem: Option<RootedFileSystem>,
    upload_permission: bool,
    download_permission: bool,
    uploads: TransferRegistry<IncomingTransfer>,
    downloads: TransferRegistry<OutgoingTransfer>,
}

#[cfg(windows)]
struct AgentSessionContext<'a> {
    session_id: SessionId,
    frames_per_second: u32,
    view_permission: bool,
    outbound_cipher: &'a XChaChaSessionCipher,
    inbound_cipher: &'a XChaChaSessionCipher,
}

#[cfg(windows)]
#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?;

    if let Some(config) = load_managed_config()? {
        return run_managed_agent(config).await;
    }
    run_agent_session(load_config()?).await.map(|_| ())
}

#[cfg(windows)]
#[allow(clippy::too_many_lines)]
async fn run_agent_session(config: AgentConfig) -> anyhow::Result<u64> {
    run_agent_session_with_report(config, None).await
}

#[cfg(windows)]
#[allow(clippy::too_many_lines)]
async fn run_agent_session_with_report(
    config: AgentConfig,
    connection_report: Option<oneshot::Sender<ConnectionType>>,
) -> anyhow::Result<u64> {
    let AgentConfig {
        relay_address,
        server_name,
        certificate_path,
        session_id,
        token,
        end_to_end_key,
        frames_per_second,
        view_permission,
        input_permission,
        clipboard_permission,
        file_upload_permission,
        file_download_permission,
        file_roots,
        direct,
    } = config;
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
        view_desktop = view_permission,
        control_input = input_permission,
        clipboard = clipboard_permission,
        file_upload = file_upload_permission,
        file_download = file_download_permission,
        "local M7 permissions loaded"
    );
    let mut clipboard = PermissionedClipboard::new(
        WindowsClipboardBackend,
        ClipboardOrigin::Agent,
        clipboard_permission,
    );
    let filesystem = if file_roots.is_empty() {
        None
    } else {
        Some(RootedFileSystem::new(file_roots)?)
    };
    let file_service = AgentFileService {
        filesystem,
        upload_permission: file_upload_permission,
        download_permission: file_download_permission,
        uploads: TransferRegistry::default(),
        downloads: TransferRegistry::default(),
    };

    let client_endpoint = client_endpoint(Path::new(&certificate_path))?;
    let connection = client_endpoint
        .connect(relay_address, &server_name)
        .context("create relay connection")?
        .await
        .context("connect to relay")?;
    let (send, receive) = connection.open_bi().await.context("open relay stream")?;
    let mut relay_transport = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
    let hello = RelayClientMessage::ClientHello(remotex_protocol::ClientHello::new(
        session_id,
        Role::Agent,
        token,
    ));
    relay_transport
        .send(Bytes::from(encode_wire(&hello)?))
        .await?;
    wait_for_peer(&mut relay_transport, session_id).await?;
    let mut direct_endpoint = None;
    let (mut transport, connection_type): (Box<dyn Connection>, ConnectionType) =
        if let Some(settings) = direct {
            match accept_direct_connection(&settings, session_id, &end_to_end_key).await {
                Ok((endpoint, connection, connection_type)) => {
                    direct_endpoint = Some(endpoint);
                    (Box::new(connection), connection_type)
                }
                Err(error) => {
                    warn!(event = "direct_connection_failed", %session_id, %error);
                    (Box::new(relay_transport), ConnectionType::Relay)
                }
            }
        } else {
            (Box::new(relay_transport), ConnectionType::Relay)
        };
    if let Some(report) = connection_report {
        let _result = report.send(connection_type);
    }
    info!(%session_id, %relay_address, ?connection_type, "remote session active");
    windows_authorization::set_active_session_title(Some(session_id));

    let session_result = run_active_session(
        transport.as_mut(),
        &mut capture,
        &mut input,
        &mut clipboard,
        file_service,
        AgentSessionContext {
            session_id,
            frames_per_second,
            view_permission,
            outbound_cipher: &outbound_cipher,
            inbound_cipher: &inbound_cipher,
        },
    )
    .await;
    windows_authorization::set_active_session_title(None);
    let release_result = input.release_all();
    let _result = transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::Close)?))
        .await;
    let capture_result = capture.stop();
    let close_result = transport.close().await;
    client_endpoint.close(0_u32.into(), b"agent stopped");
    if let Some(endpoint) = direct_endpoint {
        endpoint.close(0_u32.into(), b"direct Session stopped");
    }
    let bytes_transferred = session_result?;
    release_result?;
    capture_result?;
    close_result?;
    Ok(bytes_transferred)
}

#[cfg(windows)]
async fn run_managed_agent(config: ManagedAgentConfig) -> anyhow::Result<()> {
    let identity = Arc::new(load_or_create_identity(&config.identity_path)?);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("build control client")?;
    let registration = DeviceRegistrationRequest {
        public_key: identity.public_bytes().to_vec(),
        device_name: config.device_name.clone(),
        platform: DevicePlatform::Windows,
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        capabilities: config.local_permissions,
    };
    let response = client
        .post(format!("{}/api/devices/register", config.control_url))
        .json(&registration)
        .send()
        .await
        .context("register device")?
        .error_for_status()
        .context("control server rejected device registration")?
        .json::<DeviceRegistrationResponse>()
        .await
        .context("decode device registration")?;
    let device_id = response.device_id;
    info!(%device_id, "device registered with control server");

    let nonce = Arc::new(AtomicU64::new(now_ms()?));
    let connectivity_candidates = config
        .direct
        .as_ref()
        .map_or_else(Vec::new, |direct| direct.candidates.clone());
    let heartbeat_task = tokio::spawn(run_managed_heartbeats(
        client.clone(),
        config.control_url.clone(),
        device_id.clone(),
        Arc::clone(&identity),
        Arc::clone(&nonce),
        config.local_permissions,
        connectivity_candidates,
    ));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            () = tokio::time::sleep(Duration::from_secs(2)) => {
                let proof = signed_proof(
                    &identity,
                    "claim_session",
                    &device_id,
                    &nonce,
                )?;
                let response = client
                    .post(format!(
                        "{}/api/devices/{device_id}/sessions/claim",
                        config.control_url
                    ))
                    .json(&ClaimAgentSessionRequest { proof })
                    .send()
                    .await
                    .context("claim pending session")?;
                if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                    warn!(event = "control_device_auth_rejected", %device_id);
                    continue;
                }
                let claimed = response
                    .error_for_status()
                    .context("control server rejected Session claim")?
                    .json::<ClaimAgentSessionResponse>()
                    .await
                    .context("decode claimed Session")?;
                if let Some(incoming) = claimed.authorization_request {
                    handle_incoming_session(
                        &config,
                        &client,
                        &device_id,
                        &identity,
                        &nonce,
                        incoming,
                    )
                    .await?;
                }
            }
        }
    }
    heartbeat_task.abort();
    Ok(())
}

#[cfg(windows)]
async fn handle_incoming_session(
    config: &ManagedAgentConfig,
    client: &reqwest::Client,
    device_id: &DeviceId,
    identity: &Ed25519DeviceIdentity,
    nonce: &AtomicU64,
    incoming: IncomingSessionRequest,
) -> anyhow::Result<()> {
    let granted_permissions = incoming
        .requested_permissions
        .intersect(config.local_permissions);
    let accepted = authorize_incoming_session(
        config.unattended_access,
        config.unattended_secret.as_deref(),
        incoming.clone(),
        granted_permissions,
    )
    .await?;
    let action = format!("authorize_session:{}", incoming.session_id);
    let proof = signed_proof(identity, &action, device_id, nonce)?;
    let authorization = client
        .post(format!(
            "{}/api/devices/{device_id}/sessions/{}/authorize",
            config.control_url, incoming.session_id,
        ))
        .json(&ResolveSessionAuthorizationRequest {
            proof,
            decision: if accepted {
                AuthorizationDecision::Accept
            } else {
                AuthorizationDecision::Reject
            },
            granted_permissions,
        })
        .send()
        .await
        .context("resolve incoming Session authorization")?
        .error_for_status()
        .context("control server rejected Session authorization")?
        .json::<ResolveSessionAuthorizationResponse>()
        .await
        .context("decode Session authorization response")?;
    let Some(credentials) = authorization.credentials else {
        info!(session_id = %incoming.session_id, "incoming remote Session rejected");
        return Ok(());
    };
    let session_id = credentials.session_id;
    let session = managed_session_config(config, credentials)?;
    let (connection_sender, connection_receiver) = oneshot::channel();
    let session_future = run_agent_session_with_report(session, Some(connection_sender));
    tokio::pin!(session_future);
    let connection_type = tokio::select! {
        selected = connection_receiver => selected.context("Session ended before selecting a transport")?,
        result = &mut session_future => {
            return result.map(|_| ());
        }
    };
    report_managed_session_event(
        client,
        &config.control_url,
        device_id,
        identity,
        nonce,
        session_id,
        SessionAuditEventKind::Started,
        connection_type,
        0,
        "active",
    )
    .await?;
    let (bytes_transferred, result) = match session_future.await {
        Ok(bytes_transferred) => (bytes_transferred, "disconnected"),
        Err(error) => {
            warn!(event = "managed_session_ended_with_error", %error);
            (0, "error")
        }
    };
    report_managed_session_event(
        client,
        &config.control_url,
        device_id,
        identity,
        nonce,
        session_id,
        SessionAuditEventKind::Ended,
        connection_type,
        bytes_transferred,
        result,
    )
    .await
}

#[cfg(windows)]
async fn run_managed_heartbeats(
    client: reqwest::Client,
    control_url: String,
    device_id: DeviceId,
    identity: Arc<Ed25519DeviceIdentity>,
    nonce: Arc<AtomicU64>,
    capabilities: SessionPermissions,
    connectivity_candidates: Vec<ConnectivityCandidate>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let request = match signed_proof(&identity, "heartbeat", &device_id, &nonce) {
            Ok(proof) => DeviceHeartbeatRequest {
                proof,
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
                platform: DevicePlatform::Windows,
                capabilities,
                connectivity_candidates: connectivity_candidates.clone(),
            },
            Err(error) => {
                warn!(event = "heartbeat_sign_failed", %error);
                continue;
            }
        };
        match client
            .post(format!("{control_url}/api/devices/{device_id}/heartbeat"))
            .json(&request)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => warn!(
                event = "control_heartbeat_rejected",
                status = %response.status(),
                %device_id
            ),
            Err(error) => warn!(event = "control_heartbeat_failed", %error, %device_id),
        }
    }
}

#[cfg(windows)]
async fn authorize_incoming_session(
    unattended_access: bool,
    unattended_secret: Option<&str>,
    incoming: IncomingSessionRequest,
    granted_permissions: SessionPermissions,
) -> anyhow::Result<bool> {
    if unattended_authorization(
        unattended_access,
        unattended_secret,
        incoming.unattended_secret.as_deref(),
    )
    .is_some()
    {
        info!(
            session_id = %incoming.session_id,
            controller = %incoming.controller_name,
            "incoming Session accepted by explicitly enabled unattended access"
        );
        return Ok(true);
    }
    tokio::task::spawn_blocking(move || {
        windows_authorization::confirm_incoming_session(&incoming, granted_permissions)
    })
    .await
    .context("join local authorization prompt")
}

#[cfg(windows)]
fn unattended_authorization(
    enabled: bool,
    configured_secret: Option<&str>,
    presented_secret: Option<&str>,
) -> Option<bool> {
    if enabled
        && configured_secret
            .zip(presented_secret)
            .is_some_and(|(configured, presented)| {
                secrets_equal(configured.as_bytes(), presented.as_bytes())
            })
    {
        Some(true)
    } else {
        None
    }
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
async fn report_managed_session_event(
    client: &reqwest::Client,
    control_url: &str,
    device_id: &DeviceId,
    identity: &Ed25519DeviceIdentity,
    nonce: &AtomicU64,
    session_id: SessionId,
    kind: SessionAuditEventKind,
    connection_type: ConnectionType,
    bytes_transferred: u64,
    result: &str,
) -> anyhow::Result<()> {
    let kind_text = match kind {
        SessionAuditEventKind::Started => "started",
        SessionAuditEventKind::Ended => "ended",
    };
    let action = format!("session_event:{session_id}:{kind_text}");
    let request = ReportSessionEventRequest {
        proof: signed_proof(identity, &action, device_id, nonce)?,
        kind,
        connection_type,
        bytes_transferred,
        result: result.to_owned(),
    };
    client
        .post(format!(
            "{control_url}/api/devices/{device_id}/sessions/{session_id}/events"
        ))
        .json(&request)
        .send()
        .await
        .context("report managed Session event")?
        .error_for_status()
        .context("control server rejected Session event")?;
    Ok(())
}

#[cfg(windows)]
fn managed_session_config(
    config: &ManagedAgentConfig,
    credentials: SessionCredentials,
) -> anyhow::Result<AgentConfig> {
    if credentials.expires_at_ms <= now_ms()? {
        anyhow::bail!("claimed Session credentials have expired");
    }
    Ok(AgentConfig {
        relay_address: credentials
            .relay_address
            .parse()
            .context("parse managed Relay address")?,
        server_name: credentials.relay_server_name,
        certificate_path: config.relay_certificate_path.clone(),
        session_id: credentials.session_id,
        token: parse_token(&credentials.role_token_hex)?,
        end_to_end_key: parse_key(&credentials.end_to_end_key_hex)?,
        frames_per_second: config.frames_per_second,
        view_permission: credentials.permissions.view_desktop
            && config.local_permissions.view_desktop,
        input_permission: credentials.permissions.control_input
            && config.local_permissions.control_input,
        clipboard_permission: credentials.permissions.clipboard
            && config.local_permissions.clipboard,
        file_upload_permission: credentials.permissions.file_upload
            && config.local_permissions.file_upload,
        file_download_permission: credentials.permissions.file_download
            && config.local_permissions.file_download,
        file_roots: config.file_roots.clone(),
        direct: config.direct.clone(),
    })
}

#[cfg(windows)]
fn signed_proof(
    identity: &Ed25519DeviceIdentity,
    action: &str,
    device_id: &DeviceId,
    last_nonce: &AtomicU64,
) -> anyhow::Result<DeviceAuthProof> {
    let timestamp_ms = now_ms()?;
    let nonce = last_nonce
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |previous| {
            Some(timestamp_ms.max(previous.saturating_add(1)))
        })
        .map_err(|_| anyhow::anyhow!("device proof nonce update failed"))?
        .saturating_add(1)
        .max(timestamp_ms);
    Ok(DeviceAuthProof {
        timestamp_ms,
        nonce,
        signature: identity.sign(&device_auth_message(
            action,
            device_id.as_str(),
            timestamp_ms,
            nonce,
        ))?,
    })
}

#[cfg(windows)]
fn load_or_create_identity(path: &Path) -> anyhow::Result<Ed25519DeviceIdentity> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let stored: StoredIdentity =
                serde_json::from_slice(&bytes).context("decode device identity")?;
            let secret: [u8; 32] = hex::decode(stored.secret_key_hex)
                .context("decode device identity secret")?
                .try_into()
                .map_err(|_| anyhow::anyhow!("device identity secret must contain 32 bytes"))?;
            Ok(Ed25519DeviceIdentity::from_secret_bytes(secret))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("create device identity directory {}", parent.display())
                })?;
            }
            let identity = Ed25519DeviceIdentity::generate();
            let bytes = serde_json::to_vec(&StoredIdentity {
                secret_key_hex: hex::encode(identity.secret_bytes()),
            })?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)
                .with_context(|| format!("create device identity {}", path.display()))?;
            std::io::Write::write_all(&mut file, &bytes).context("write device identity")?;
            Ok(identity)
        }
        Err(error) => {
            Err(error).with_context(|| format!("read device identity {}", path.display()))
        }
    }
}

#[cfg(windows)]
async fn run_active_session(
    transport: &mut dyn Connection,
    capture: &mut DxgiCapture,
    input: &mut impl InputController,
    clipboard: &mut PermissionedClipboard<WindowsClipboardBackend>,
    file_service: AgentFileService,
    context: AgentSessionContext<'_>,
) -> anyhow::Result<u64> {
    let mut codec = SessionVideoEncoder::new(context.frames_per_second)?;
    let mut outbound_sequence = 0_u64;
    let mut expected_inbound_sequence = 0_u64;
    let mut capture_interval = tokio::time::interval(Duration::from_millis(
        1000 / u64::from(context.frames_per_second),
    ));
    capture_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut clipboard_interval = tokio::time::interval(Duration::from_millis(500));
    clipboard_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let (file_command_sender, file_command_receiver) = mpsc::channel(FILE_COMMAND_QUEUE_CAPACITY);
    let (file_response_sender, mut file_response_receiver) =
        mpsc::channel(FILE_RESPONSE_QUEUE_CAPACITY);
    let file_worker = tokio::spawn(file_service.run(file_command_receiver, file_response_sender));
    let mut bytes_transferred = 0_u64;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = capture_interval.tick(), if context.view_permission => {
                let capture_started = Instant::now();
                let frame = match capture.next_frame() {
                    Ok(frame) => frame,
                    Err(CaptureError::Timeout) => continue,
                    Err(error) => return Err(error.into()),
                };
                if !codec.frame_due(frame.timestamp_ms) {
                    continue;
                }
                let capture_latency_ms = u32::try_from(capture_started.elapsed().as_millis())
                    .unwrap_or(u32::MAX);
                let video = codec.encode(&frame, capture_latency_ms)?;
                bytes_transferred = bytes_transferred.saturating_add(send_agent_message(
                    transport,
                    context.session_id,
                    context.outbound_cipher,
                    &mut outbound_sequence,
                    frame.timestamp_ms,
                    Message::Video(video),
                ).await?);
            }
            _ = clipboard_interval.tick(), if clipboard.is_enabled() => {
                match clipboard.poll() {
                    Ok(Some(message)) => {
                        bytes_transferred = bytes_transferred.saturating_add(send_agent_message(
                            transport,
                            context.session_id,
                            context.outbound_cipher,
                            &mut outbound_sequence,
                            now_ms()?,
                            Message::Clipboard(message),
                        ).await?);
                    }
                    Ok(None) => {}
                    Err(error) => warn!(event = "clipboard_poll_failed", %error),
                }
            }
            response = file_response_receiver.recv() => {
                let Some(response) = response else {
                    anyhow::bail!("file service stopped unexpectedly");
                };
                bytes_transferred = bytes_transferred.saturating_add(send_agent_message(
                    transport,
                    context.session_id,
                    context.outbound_cipher,
                    &mut outbound_sequence,
                    now_ms()?,
                    Message::FileTransfer(response),
                ).await?);
            }
            incoming = transport.receive() => {
                let incoming = incoming?;
                bytes_transferred = bytes_transferred.saturating_add(
                    u64::try_from(incoming.len()).unwrap_or(u64::MAX)
                );
                handle_relay_message(
                    transport,
                    decode_wire(&incoming)?,
                    context.session_id,
                    context.inbound_cipher,
                    &mut expected_inbound_sequence,
                    input,
                    clipboard,
                    &file_command_sender,
                    &mut codec,
                ).await?;
            }
        }
    }
    drop(file_command_sender);
    file_worker.abort();
    Ok(bytes_transferred)
}

#[cfg(windows)]
async fn send_agent_message(
    transport: &mut dyn Connection,
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    sequence: &mut u64,
    timestamp_ms: u64,
    message: Message,
) -> anyhow::Result<u64> {
    let envelope = MessageEnvelope::new(session_id, *sequence, timestamp_ms, message);
    let plaintext = encode_wire(&envelope).context("encode Agent envelope")?;
    let ciphertext = cipher
        .seal(*sequence, &plaintext)
        .context("encrypt Agent envelope")?;
    let mut payload = Vec::with_capacity(8 + ciphertext.len());
    payload.extend_from_slice(&(*sequence).to_be_bytes());
    payload.extend_from_slice(&ciphertext);
    let encoded = encode_wire(&RelayClientMessage::Payload(payload))?;
    let byte_count = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
    transport.send(Bytes::from(encoded)).await?;
    *sequence = sequence
        .checked_add(1)
        .context("Agent outbound sequence space exhausted")?;
    Ok(byte_count)
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
        .unwrap_or_else(|_| "30".to_owned())
        .parse()
        .context("parse REMOTEX_VIDEO_FPS")?;
    if !(1..=30).contains(&frames_per_second) {
        anyhow::bail!("REMOTEX_VIDEO_FPS must be between 1 and 30");
    }
    let file_upload_permission = parse_switch("REMOTEX_ALLOW_FILE_UPLOAD", false)?;
    let file_download_permission = parse_switch("REMOTEX_ALLOW_FILE_DOWNLOAD", false)?;
    let file_roots = parse_file_roots(std::env::var("REMOTEX_FILE_ROOTS").ok().as_deref())?;
    if (file_upload_permission || file_download_permission) && file_roots.is_empty() {
        anyhow::bail!(
            "REMOTEX_FILE_ROOTS must configure at least one Name=Path root when file access is enabled"
        );
    }
    Ok(AgentConfig {
        relay_address,
        server_name: required("REMOTEX_RELAY_SERVER_NAME")?,
        certificate_path: required("REMOTEX_RELAY_CA_CERT")?,
        session_id,
        token: parse_token(&required("REMOTEX_AGENT_TOKEN_HEX")?)?,
        end_to_end_key: parse_key(&required("REMOTEX_E2E_KEY_HEX")?)?,
        frames_per_second,
        view_permission: true,
        input_permission: parse_switch("REMOTEX_ALLOW_INPUT", false)?,
        clipboard_permission: parse_switch("REMOTEX_ALLOW_CLIPBOARD", false)?,
        file_upload_permission,
        file_download_permission,
        file_roots,
        direct: None,
    })
}

#[cfg(windows)]
fn load_managed_config() -> anyhow::Result<Option<ManagedAgentConfig>> {
    let control_url = match std::env::var("REMOTEX_CONTROL_URL") {
        Ok(value) => value.trim_end_matches('/').to_owned(),
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(error).context("read REMOTEX_CONTROL_URL"),
    };
    if control_url.is_empty() {
        anyhow::bail!("REMOTEX_CONTROL_URL must not be empty");
    }
    let frames_per_second = std::env::var("REMOTEX_VIDEO_FPS")
        .unwrap_or_else(|_| "30".to_owned())
        .parse()
        .context("parse REMOTEX_VIDEO_FPS")?;
    if !(1..=30).contains(&frames_per_second) {
        anyhow::bail!("REMOTEX_VIDEO_FPS must be between 1 and 30");
    }
    let local_permissions = SessionPermissions {
        view_desktop: true,
        control_input: parse_switch("REMOTEX_ALLOW_INPUT", false)?,
        clipboard: parse_switch("REMOTEX_ALLOW_CLIPBOARD", false)?,
        file_upload: parse_switch("REMOTEX_ALLOW_FILE_UPLOAD", false)?,
        file_download: parse_switch("REMOTEX_ALLOW_FILE_DOWNLOAD", false)?,
    };
    let file_roots = parse_file_roots(std::env::var("REMOTEX_FILE_ROOTS").ok().as_deref())?;
    if (local_permissions.file_upload || local_permissions.file_download) && file_roots.is_empty() {
        anyhow::bail!(
            "REMOTEX_FILE_ROOTS must configure at least one Name=Path root when file access is enabled"
        );
    }
    let unattended_access = parse_switch("REMOTEX_UNATTENDED_ACCESS", false)?;
    let unattended_secret = if unattended_access {
        let secret = required("REMOTEX_UNATTENDED_SECRET")?;
        if !(12..=128).contains(&secret.len()) {
            anyhow::bail!("REMOTEX_UNATTENDED_SECRET must contain 12 to 128 bytes");
        }
        Some(secret)
    } else {
        None
    };
    let direct = load_direct_settings()?;
    Ok(Some(ManagedAgentConfig {
        control_url,
        identity_path: PathBuf::from(required("REMOTEX_IDENTITY_PATH")?),
        relay_certificate_path: required("REMOTEX_RELAY_CA_CERT")?,
        device_name: std::env::var("REMOTEX_DEVICE_NAME")
            .unwrap_or_else(|_| "Windows PC".to_owned()),
        frames_per_second,
        local_permissions,
        unattended_access,
        unattended_secret,
        file_roots,
        direct,
    }))
}

#[cfg(windows)]
fn load_direct_settings() -> anyhow::Result<Option<DirectServerSettings>> {
    let certificate = std::env::var("REMOTEX_DIRECT_CERT");
    let private_key = std::env::var("REMOTEX_DIRECT_KEY");
    let (certificate, private_key) = match (certificate, private_key) {
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => {
            return Ok(None);
        }
        (Ok(certificate), Ok(private_key)) => (certificate, private_key),
        _ => {
            anyhow::bail!("REMOTEX_DIRECT_CERT and REMOTEX_DIRECT_KEY must be configured together")
        }
    };
    let bind: SocketAddr = std::env::var("REMOTEX_DIRECT_BIND")
        .unwrap_or_else(|_| "0.0.0.0:7444".to_owned())
        .parse()
        .context("parse REMOTEX_DIRECT_BIND")?;
    if bind.port() == 0 {
        anyhow::bail!("REMOTEX_DIRECT_BIND must use a fixed nonzero port");
    }
    let server_name = required("REMOTEX_DIRECT_SERVER_NAME")?;
    let mut candidates = Vec::new();
    let lan_address = match std::env::var("REMOTEX_DIRECT_LAN_ADDRESS") {
        Ok(value) => Some(value.parse().context("parse REMOTEX_DIRECT_LAN_ADDRESS")?),
        Err(std::env::VarError::NotPresent) => discover_lan_address(bind.port()),
        Err(error) => return Err(error).context("read REMOTEX_DIRECT_LAN_ADDRESS"),
    };
    if let Some(address) = lan_address {
        candidates.push(ConnectivityCandidate {
            kind: ConnectivityCandidateKind::Lan,
            address: address.to_string(),
            server_name: server_name.clone(),
            priority: 200,
        });
    }
    match std::env::var("REMOTEX_DIRECT_PUBLIC_ADDRESS") {
        Ok(value) => {
            let address: SocketAddr = value
                .parse()
                .context("parse REMOTEX_DIRECT_PUBLIC_ADDRESS")?;
            candidates.push(ConnectivityCandidate {
                kind: ConnectivityCandidateKind::ServerReflexive,
                address: address.to_string(),
                server_name,
                priority: 100,
            });
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => return Err(error).context("read REMOTEX_DIRECT_PUBLIC_ADDRESS"),
    }
    if candidates.is_empty() {
        anyhow::bail!(
            "direct connectivity needs a discovered LAN address or REMOTEX_DIRECT_PUBLIC_ADDRESS"
        );
    }
    for candidate in &candidates {
        candidate.validate()?;
    }
    Ok(Some(DirectServerSettings {
        bind,
        certificate_path: PathBuf::from(certificate),
        private_key_path: PathBuf::from(private_key),
        candidates,
    }))
}

#[cfg(windows)]
fn discover_lan_address(port: u16) -> Option<SocketAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?;
    (!address.ip().is_unspecified() && !address.ip().is_loopback())
        .then(|| SocketAddr::new(address.ip(), port))
}

#[cfg(windows)]
fn parse_file_roots(value: Option<&str>) -> anyhow::Result<Vec<AllowedRoot>> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    value
        .split(';')
        .map(|entry| {
            let (name, path) = entry
                .split_once('=')
                .context("each REMOTEX_FILE_ROOTS entry must use Name=Path")?;
            if name.trim().is_empty() || path.trim().is_empty() {
                anyhow::bail!("REMOTEX_FILE_ROOTS names and paths must not be empty");
            }
            Ok(AllowedRoot {
                name: name.trim().to_owned(),
                path: PathBuf::from(path.trim()),
            })
        })
        .collect()
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
#[allow(clippy::too_many_arguments)]
async fn handle_relay_message(
    transport: &mut dyn Connection,
    message: RelayServerMessage,
    session_id: SessionId,
    inbound_cipher: &XChaChaSessionCipher,
    expected_sequence: &mut u64,
    input: &mut impl InputController,
    clipboard: &mut PermissionedClipboard<WindowsClipboardBackend>,
    file_commands: &mpsc::Sender<FileTransferMessage>,
    video: &mut SessionVideoEncoder,
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
            if let Some(file_message) = apply_controller_payload(
                &payload,
                session_id,
                inbound_cipher,
                expected_sequence,
                input,
                clipboard,
                video,
            )? {
                file_commands
                    .try_send(file_message)
                    .map_err(|_| anyhow::anyhow!("file command queue is full or closed"))?;
            }
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
    video: &mut SessionVideoEncoder,
) -> anyhow::Result<Option<FileTransferMessage>> {
    if payload.len() > MAX_FILE_CHUNK_SIZE as usize + 64 * 1024 {
        anyhow::bail!("Controller data payload exceeds the M7 limit");
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
        Message::FileTransfer(message) => {
            return finish_inbound_sequence(expected_sequence, Some(message));
        }
        Message::Control(ControlMessage::VideoCapabilities { codecs }) => {
            let selected = video.negotiate(&codecs)?;
            info!(?selected, %session_id, "negotiated video codec");
        }
        Message::Control(ControlMessage::VideoFeedback(feedback)) => {
            if video.apply_feedback(feedback)? {
                info!(profile = ?video.profile(), %session_id, "adapted video quality");
            }
        }
        _ => {
            anyhow::bail!("Controller sent a message not allowed in its data direction");
        }
    }
    finish_inbound_sequence(expected_sequence, None)
}

#[cfg(windows)]
fn finish_inbound_sequence(
    expected_sequence: &mut u64,
    message: Option<FileTransferMessage>,
) -> anyhow::Result<Option<FileTransferMessage>> {
    *expected_sequence = expected_sequence
        .checked_add(1)
        .context("Controller inbound sequence space exhausted")?;
    Ok(message)
}

#[cfg(windows)]
impl AgentFileService {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<FileTransferMessage>,
        responses: mpsc::Sender<FileTransferMessage>,
    ) {
        while let Some(command) = commands.recv().await {
            for response in self.handle(command).await {
                if responses.send(response).await.is_err() {
                    return;
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn handle(&mut self, message: FileTransferMessage) -> Vec<FileTransferMessage> {
        match message {
            FileTransferMessage::ListDirectoryRequest { request_id, path } => {
                if !self.upload_permission && !self.download_permission {
                    return vec![file_error(
                        Some(request_id),
                        None,
                        FileTransferErrorCode::PermissionDenied,
                    )];
                }
                let result = match self.filesystem.as_ref() {
                    Some(filesystem) => filesystem.list_directory(&path).await,
                    None => {
                        return vec![file_error(
                            Some(request_id),
                            None,
                            FileTransferErrorCode::PermissionDenied,
                        )];
                    }
                };
                match result {
                    Ok(entries) => vec![FileTransferMessage::ListDirectoryResponse {
                        request_id,
                        path,
                        entries,
                    }],
                    Err(error) => vec![file_service_error(Some(request_id), None, &error)],
                }
            }
            FileTransferMessage::CreateDirectoryRequest { request_id, path } => {
                if !self.upload_permission {
                    return vec![file_error(
                        Some(request_id),
                        None,
                        FileTransferErrorCode::PermissionDenied,
                    )];
                }
                let result = match self.filesystem.as_ref() {
                    Some(filesystem) => filesystem.create_directory(&path).await,
                    None => {
                        return vec![file_error(
                            Some(request_id),
                            None,
                            FileTransferErrorCode::PermissionDenied,
                        )];
                    }
                };
                match result {
                    Ok(()) => {
                        vec![FileTransferMessage::CreateDirectoryResponse { request_id, path }]
                    }
                    Err(error) => vec![file_service_error(Some(request_id), None, &error)],
                }
            }
            FileTransferMessage::DownloadRequest {
                transfer_id,
                source_path,
            } => {
                if !self.download_permission {
                    return vec![file_error(
                        None,
                        Some(transfer_id),
                        FileTransferErrorCode::PermissionDenied,
                    )];
                }
                let result = match self.filesystem.as_ref() {
                    Some(filesystem) => filesystem.open_download(&source_path).await,
                    None => {
                        return vec![file_error(
                            None,
                            Some(transfer_id),
                            FileTransferErrorCode::PermissionDenied,
                        )];
                    }
                };
                match result {
                    Ok(transfer) => {
                        let response = match transfer.filename() {
                            Ok(filename) => FileTransferMessage::Start {
                                transfer_id,
                                direction: FileTransferDirection::Download,
                                filename,
                                source_path,
                                destination_path: String::new(),
                                total_size: transfer.total_size(),
                                chunk_size: transfer.chunk_size(),
                                sha256: transfer.sha256(),
                            },
                            Err(error) => {
                                return vec![file_service_error(None, Some(transfer_id), &error)];
                            }
                        };
                        match self.downloads.insert(transfer_id, transfer) {
                            Ok(()) => vec![response],
                            Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                        }
                    }
                    Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                }
            }
            FileTransferMessage::Start {
                transfer_id,
                direction: FileTransferDirection::Upload,
                destination_path,
                total_size,
                chunk_size,
                sha256,
                ..
            } => {
                if !self.upload_permission {
                    return vec![file_error(
                        None,
                        Some(transfer_id),
                        FileTransferErrorCode::PermissionDenied,
                    )];
                }
                let result = match self.filesystem.as_ref() {
                    Some(filesystem) => {
                        filesystem
                            .open_upload(
                                transfer_id,
                                &destination_path,
                                total_size,
                                chunk_size,
                                sha256,
                            )
                            .await
                    }
                    None => {
                        return vec![file_error(
                            None,
                            Some(transfer_id),
                            FileTransferErrorCode::PermissionDenied,
                        )];
                    }
                };
                match result {
                    Ok(transfer) => {
                        let next_offset = transfer.next_offset();
                        match self.uploads.insert(transfer_id, transfer) {
                            Ok(()) => vec![FileTransferMessage::Accept {
                                transfer_id,
                                next_offset,
                            }],
                            Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                        }
                    }
                    Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                }
            }
            FileTransferMessage::Chunk {
                transfer_id,
                offset,
                checksum,
                payload,
            } => {
                let result = match self.uploads.get_mut(&transfer_id) {
                    Ok(transfer) => transfer.write_chunk(offset, checksum, &payload).await,
                    Err(error) => Err(error),
                };
                match result {
                    Ok(next_offset) => vec![FileTransferMessage::ChunkAck {
                        transfer_id,
                        next_offset,
                    }],
                    Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                }
            }
            FileTransferMessage::Complete {
                transfer_id,
                total_size,
                sha256,
            } => {
                let result = match self.uploads.remove(&transfer_id) {
                    Ok(transfer) => transfer.complete(total_size, sha256).await,
                    Err(error) => Err(error),
                };
                match result {
                    Ok(()) => vec![FileTransferMessage::Complete {
                        transfer_id,
                        total_size,
                        sha256,
                    }],
                    Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
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
            } => {
                vec![self.download_chunk(transfer_id, next_offset).await]
            }
            FileTransferMessage::Cancel { transfer_id } => {
                let result = if self.uploads.contains(&transfer_id) {
                    match self.uploads.remove(&transfer_id) {
                        Ok(transfer) => transfer.cancel().await,
                        Err(error) => Err(error),
                    }
                } else if self.downloads.contains(&transfer_id) {
                    self.downloads.remove(&transfer_id).map(|_| ())
                } else {
                    Err(FileTransferError::NotFound)
                };
                match result {
                    Ok(()) => vec![FileTransferMessage::Cancel { transfer_id }],
                    Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                }
            }
            FileTransferMessage::Error {
                transfer_id: Some(transfer_id),
                ..
            } => {
                if self.uploads.contains(&transfer_id) {
                    let _ = self.uploads.remove(&transfer_id);
                }
                if self.downloads.contains(&transfer_id) {
                    let _ = self.downloads.remove(&transfer_id);
                }
                Vec::new()
            }
            FileTransferMessage::Progress { transfer_id, .. }
            | FileTransferMessage::Start { transfer_id, .. } => vec![file_error(
                None,
                Some(transfer_id),
                FileTransferErrorCode::InvalidChunk,
            )],
            FileTransferMessage::ListDirectoryResponse { request_id, .. }
            | FileTransferMessage::CreateDirectoryResponse { request_id, .. }
            | FileTransferMessage::Error {
                request_id: Some(request_id),
                transfer_id: None,
                ..
            } => vec![file_error(
                Some(request_id),
                None,
                FileTransferErrorCode::InvalidPath,
            )],
            FileTransferMessage::Error {
                request_id: None,
                transfer_id: None,
                ..
            } => Vec::new(),
        }
    }

    async fn download_chunk(
        &mut self,
        transfer_id: TransferId,
        offset: u64,
    ) -> FileTransferMessage {
        let result = match self.downloads.get_mut(&transfer_id) {
            Ok(transfer) => transfer.read_chunk(offset).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(Some((offset, checksum, payload))) => FileTransferMessage::Chunk {
                transfer_id,
                offset,
                checksum,
                payload,
            },
            Ok(None) => match self.downloads.remove(&transfer_id) {
                Ok(transfer) => FileTransferMessage::Complete {
                    transfer_id,
                    total_size: transfer.total_size(),
                    sha256: transfer.sha256(),
                },
                Err(error) => file_service_error(None, Some(transfer_id), &error),
            },
            Err(error) => file_service_error(None, Some(transfer_id), &error),
        }
    }
}

#[cfg(windows)]
fn file_service_error(
    request_id: Option<u64>,
    transfer_id: Option<TransferId>,
    error: &FileTransferError,
) -> FileTransferMessage {
    let code = match error {
        FileTransferError::PermissionDenied => FileTransferErrorCode::PermissionDenied,
        FileTransferError::NotFound | FileTransferError::UnknownRoot => {
            FileTransferErrorCode::NotFound
        }
        FileTransferError::AlreadyExists | FileTransferError::DuplicateRoot(_) => {
            FileTransferErrorCode::AlreadyExists
        }
        FileTransferError::UnexpectedOffset { .. } => FileTransferErrorCode::InvalidOffset,
        FileTransferError::InvalidChunk
        | FileTransferError::InvalidChunkSize
        | FileTransferError::ExceedsDeclaredSize
        | FileTransferError::ChunkChecksumMismatch
        | FileTransferError::Incomplete { .. } => FileTransferErrorCode::InvalidChunk,
        FileTransferError::ChecksumMismatch => FileTransferErrorCode::ChecksumMismatch,
        FileTransferError::Io(_) => FileTransferErrorCode::Io,
        _ => FileTransferErrorCode::InvalidPath,
    };
    file_error(request_id, transfer_id, code)
}

#[cfg(windows)]
fn file_error(
    request_id: Option<u64>,
    transfer_id: Option<TransferId>,
    code: FileTransferErrorCode,
) -> FileTransferMessage {
    let message = match code {
        FileTransferErrorCode::PermissionDenied => "file operation is not permitted",
        FileTransferErrorCode::InvalidPath => "remote path is invalid",
        FileTransferErrorCode::NotFound => "remote path or transfer was not found",
        FileTransferErrorCode::AlreadyExists => "destination already exists",
        FileTransferErrorCode::InvalidOffset => "transfer offset is invalid",
        FileTransferErrorCode::InvalidChunk => "file chunk is invalid",
        FileTransferErrorCode::ChecksumMismatch => "file checksum does not match",
        FileTransferErrorCode::Cancelled => "file transfer was cancelled",
        FileTransferErrorCode::Busy => "file service is busy",
        FileTransferErrorCode::Io => "remote filesystem operation failed",
    };
    FileTransferMessage::Error {
        request_id,
        transfer_id,
        code,
        message: message.to_owned(),
    }
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
async fn accept_direct_connection(
    settings: &DirectServerSettings,
    session_id: SessionId,
    session_key: &[u8; 32],
) -> anyhow::Result<(Endpoint, DirectPeerConnection, ConnectionType)> {
    let server_config =
        direct_server_config(&settings.certificate_path, &settings.private_key_path)?;
    let endpoint =
        Endpoint::server(server_config, settings.bind).context("bind direct endpoint")?;
    let (connection, remote_address) =
        tokio::time::timeout(DEFAULT_DIRECT_ATTEMPT_TIMEOUT, async {
            for _ in 0..8 {
                let incoming = endpoint.accept().await.context("direct endpoint closed")?;
                let connection = match incoming.await {
                    Ok(connection) => connection,
                    Err(error) => {
                        warn!(event = "direct_quic_handshake_failed", %error);
                        continue;
                    }
                };
                let remote_address = connection.remote_address();
                let (send, receive) = match connection.accept_bi().await {
                    Ok(streams) => streams,
                    Err(error) => {
                        warn!(event = "direct_stream_open_failed", %error);
                        continue;
                    }
                };
                let mut framed = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
                match authenticate_direct_server(&mut framed, session_id, session_key).await {
                    Ok(()) => return Ok((framed, remote_address)),
                    Err(error) => warn!(event = "direct_peer_authentication_failed", %error),
                }
            }
            anyhow::bail!("direct authentication attempt limit reached")
        })
        .await
        .context("direct connection attempt timed out")??;
    let connection_type = if is_private_address(remote_address.ip()) {
        ConnectionType::Lan
    } else {
        ConnectionType::Direct
    };
    Ok((
        endpoint,
        DirectPeerConnection::new(connection),
        connection_type,
    ))
}

#[cfg(windows)]
fn direct_server_config(certificate: &Path, private_key: &Path) -> anyhow::Result<ServerConfig> {
    let mut certificate_reader = BufReader::new(
        File::open(certificate)
            .with_context(|| format!("open direct certificate {}", certificate.display()))?,
    );
    let certificates = rustls_pemfile::certs(&mut certificate_reader)
        .collect::<Result<Vec<_>, _>>()
        .context("read direct certificate chain")?;
    if certificates.is_empty() {
        anyhow::bail!("direct certificate file contains no certificates");
    }
    let mut key_reader = BufReader::new(
        File::open(private_key)
            .with_context(|| format!("open direct private key {}", private_key.display()))?,
    );
    let key = rustls_pemfile::private_key(&mut key_reader)
        .context("read direct private key")?
        .context("direct private-key file contains no supported key")?;
    ServerConfig::with_single_cert(certificates, key).context("build direct TLS configuration")
}

#[cfg(windows)]
const fn is_private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_private() || address.is_loopback(),
        IpAddr::V6(address) => address.is_unique_local() || address.is_loopback(),
    }
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
    eprintln!("the M7 desktop agent currently supports Windows only");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_browsing_requires_an_explicit_permission() {
        let mut service = AgentFileService {
            filesystem: None,
            upload_permission: false,
            download_permission: false,
            uploads: TransferRegistry::default(),
            downloads: TransferRegistry::default(),
        };
        let response = service
            .handle(FileTransferMessage::ListDirectoryRequest {
                request_id: 4,
                path: "/".into(),
            })
            .await;
        assert!(matches!(
            response.as_slice(),
            [FileTransferMessage::Error {
                request_id: Some(4),
                code: FileTransferErrorCode::PermissionDenied,
                ..
            }]
        ));
    }

    #[test]
    fn file_roots_use_explicit_virtual_names() {
        let roots = parse_file_roots(Some("Documents=C:\\Users\\User\\Documents;Data=D:\\Data"))
            .expect("parse roots");
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].name, "Documents");
        assert_eq!(roots[1].path, PathBuf::from("D:\\Data"));
    }

    #[test]
    fn unattended_access_is_disabled_unless_explicitly_enabled() {
        assert_eq!(
            unattended_authorization(false, Some("correct secret"), Some("correct secret")),
            None
        );
        assert_eq!(unattended_authorization(true, None, None), None);
        assert_eq!(
            unattended_authorization(true, Some("correct secret"), Some("wrong secret")),
            None
        );
        assert_eq!(
            unattended_authorization(true, Some("correct secret"), Some("correct secret")),
            Some(true)
        );
    }

    #[test]
    fn local_capabilities_can_revoke_individual_requested_permissions() {
        let requested = SessionPermissions {
            view_desktop: true,
            control_input: true,
            clipboard: true,
            file_upload: true,
            file_download: true,
        };
        let local = SessionPermissions {
            view_desktop: true,
            control_input: false,
            clipboard: true,
            file_upload: false,
            file_download: true,
        };
        assert_eq!(
            requested.intersect(local),
            SessionPermissions {
                view_desktop: true,
                control_input: false,
                clipboard: true,
                file_upload: false,
                file_download: true,
            }
        );
    }
}
