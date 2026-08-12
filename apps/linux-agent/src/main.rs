//! Visible, authorized `RemoteX` Linux server Agent.

#[cfg(target_os = "linux")]
use anyhow::Context;
#[cfg(target_os = "linux")]
use bytes::Bytes;
#[cfg(target_os = "linux")]
use quinn::{ClientConfig, Endpoint};
#[cfg(target_os = "linux")]
use remotex_crypto::{
    DeviceIdentity, Ed25519DeviceIdentity, SessionCipher, SessionDirection, XChaChaSessionCipher,
    device_auth_message, secrets_equal,
};
#[cfg(target_os = "linux")]
use remotex_file_transfer::{
    AllowedRoot, FileTransferError, IncomingTransfer, OutgoingTransfer, RootedFileSystem,
    TransferRegistry,
};
#[cfg(target_os = "linux")]
use remotex_linux_agent::{
    MAX_TERMINALS_PER_SESSION, TERMINAL_OUTPUT_QUEUE, TerminalSession, system_snapshot,
};
#[cfg(target_os = "linux")]
use remotex_protocol::{
    AuthorizationDecision, ClaimAgentSessionRequest, ClaimAgentSessionResponse, ConnectionType,
    ControlMessage, DeviceAuthProof, DeviceHeartbeatRequest, DeviceId, DevicePlatform,
    DeviceRegistrationRequest, DeviceRegistrationResponse, FileTransferDirection,
    FileTransferErrorCode, FileTransferMessage, IncomingSessionRequest, MAX_FILE_CHUNK_SIZE,
    MAX_TERMINAL_DATA_SIZE, Message, MessageEnvelope, RelayClientMessage, RelayServerMessage,
    ReportSessionEventRequest, ResolveSessionAuthorizationRequest,
    ResolveSessionAuthorizationResponse, Role, SessionAuditEventKind, SessionCredentials,
    SessionId, SessionPermissions, SessionToken, SystemMessage, TerminalId, TerminalMessage,
    TransferId, decode_wire, encode_wire,
};
#[cfg(target_os = "linux")]
use remotex_transport::{Connection, DEFAULT_MAX_FRAME_SIZE, QuicFrameConnection};
#[cfg(target_os = "linux")]
use rustls::RootCertStore;
#[cfg(target_os = "linux")]
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(target_os = "linux")]
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
#[cfg(target_os = "linux")]
use tokio::sync::mpsc;
#[cfg(target_os = "linux")]
use tokio::time::MissedTickBehavior;
#[cfg(target_os = "linux")]
use tracing::{info, warn};
#[cfg(target_os = "linux")]
use tracing_subscriber::EnvFilter;

#[cfg(target_os = "linux")]
#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    secret_key_hex: String,
}

#[cfg(target_os = "linux")]
struct Config {
    control_url: String,
    identity_path: PathBuf,
    relay_certificate_path: PathBuf,
    device_name: String,
    permissions: SessionPermissions,
    unattended_access: bool,
    unattended_secret: Option<String>,
    file_roots: Vec<AllowedRoot>,
}

#[cfg(target_os = "linux")]
struct FileService {
    filesystem: Option<RootedFileSystem>,
    upload_permission: bool,
    download_permission: bool,
    uploads: TransferRegistry<IncomingTransfer>,
    downloads: TransferRegistry<OutgoingTransfer>,
}

#[cfg(target_os = "linux")]
struct ActiveCapabilities {
    permissions: SessionPermissions,
    files: FileService,
    terminals: HashMap<TerminalId, TerminalSession>,
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .map_err(|error| anyhow::anyhow!("initialize tracing: {error}"))?;
    run(load_config()?).await
}

#[cfg(target_os = "linux")]
async fn run(config: Config) -> anyhow::Result<()> {
    let identity = Arc::new(load_or_create_identity(&config.identity_path)?);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("build control client")?;
    let registration = client
        .post(format!("{}/api/devices/register", config.control_url))
        .json(&DeviceRegistrationRequest {
            public_key: identity.public_bytes().to_vec(),
            device_name: config.device_name.clone(),
            platform: DevicePlatform::Linux,
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: config.permissions,
        })
        .send()
        .await
        .context("register Linux device")?
        .error_for_status()
        .context("control server rejected Linux device registration")?
        .json::<DeviceRegistrationResponse>()
        .await
        .context("decode Linux device registration")?;
    let device_id = registration.device_id;
    info!(%device_id, "Linux Agent enrolled and visible");
    let nonce = Arc::new(AtomicU64::new(now_ms()?));
    let heartbeat = tokio::spawn(run_heartbeats(
        client.clone(),
        config.control_url.clone(),
        device_id.clone(),
        Arc::clone(&identity),
        Arc::clone(&nonce),
        config.permissions,
    ));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            () = tokio::time::sleep(Duration::from_secs(2)) => {
                if let Some(incoming) = claim_session(&client, &config.control_url, &device_id, &identity, &nonce).await? {
                    handle_incoming(&config, &client, &device_id, &identity, &nonce, incoming).await?;
                }
            }
        }
    }
    heartbeat.abort();
    Ok(())
}

#[cfg(target_os = "linux")]
async fn claim_session(
    client: &reqwest::Client,
    control_url: &str,
    device_id: &DeviceId,
    identity: &Ed25519DeviceIdentity,
    nonce: &AtomicU64,
) -> anyhow::Result<Option<IncomingSessionRequest>> {
    let response = client
        .post(format!(
            "{control_url}/api/devices/{device_id}/sessions/claim"
        ))
        .json(&ClaimAgentSessionRequest {
            proof: signed_proof(identity, "claim_session", device_id, nonce)?,
        })
        .send()
        .await
        .context("claim Linux Session")?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        warn!(event = "control_device_auth_rejected", %device_id);
        return Ok(None);
    }
    Ok(response
        .error_for_status()
        .context("control server rejected Linux Session claim")?
        .json::<ClaimAgentSessionResponse>()
        .await
        .context("decode Linux Session claim")?
        .authorization_request)
}

#[cfg(target_os = "linux")]
async fn handle_incoming(
    config: &Config,
    client: &reqwest::Client,
    device_id: &DeviceId,
    identity: &Ed25519DeviceIdentity,
    nonce: &AtomicU64,
    incoming: IncomingSessionRequest,
) -> anyhow::Result<()> {
    let permissions = incoming.requested_permissions.intersect(config.permissions);
    let accepted = authorize(config, &incoming, permissions).await?;
    let action = format!("authorize_session:{}", incoming.session_id);
    let authorization = client
        .post(format!(
            "{}/api/devices/{device_id}/sessions/{}/authorize",
            config.control_url, incoming.session_id
        ))
        .json(&ResolveSessionAuthorizationRequest {
            proof: signed_proof(identity, &action, device_id, nonce)?,
            decision: if accepted {
                AuthorizationDecision::Accept
            } else {
                AuthorizationDecision::Reject
            },
            granted_permissions: permissions,
        })
        .send()
        .await
        .context("resolve Linux Session authorization")?
        .error_for_status()
        .context("control server rejected Linux authorization")?
        .json::<ResolveSessionAuthorizationResponse>()
        .await
        .context("decode Linux authorization response")?;
    let Some(credentials) = authorization.credentials else {
        info!(session_id = %incoming.session_id, "Linux Session rejected locally");
        return Ok(());
    };
    let session_id = credentials.session_id;
    report_event(
        client,
        config,
        device_id,
        identity,
        nonce,
        session_id,
        SessionAuditEventKind::Started,
        0,
        "active",
    )
    .await?;
    eprintln!("\n=== RemoteX Linux remote Session ACTIVE ({session_id}) ===");
    eprintln!(
        "Press Ctrl+C to disconnect. Terminal, files and system access remain permission-gated.\n"
    );
    let (bytes, result) = match run_session(config, credentials).await {
        Ok(bytes) => (bytes, "disconnected"),
        Err(error) => {
            warn!(event = "linux_session_error", %error);
            (0, "error")
        }
    };
    report_event(
        client,
        config,
        device_id,
        identity,
        nonce,
        session_id,
        SessionAuditEventKind::Ended,
        bytes,
        result,
    )
    .await
}

#[cfg(target_os = "linux")]
async fn authorize(
    config: &Config,
    incoming: &IncomingSessionRequest,
    permissions: SessionPermissions,
) -> anyhow::Result<bool> {
    if config.unattended_access
        && config
            .unattended_secret
            .as_deref()
            .zip(incoming.unattended_secret.as_deref())
            .is_some_and(|(expected, presented)| {
                secrets_equal(expected.as_bytes(), presented.as_bytes())
            })
    {
        return Ok(true);
    }
    let controller = incoming.controller_name.clone();
    tokio::task::spawn_blocking(move || {
        eprintln!("\nRemoteX: {controller} requests access");
        eprintln!("  [{}] Terminal", mark(permissions.terminal));
        eprintln!("  [{}] System information", mark(permissions.system_info));
        eprintln!("  [{}] File upload", mark(permissions.file_upload));
        eprintln!("  [{}] File download", mark(permissions.file_download));
        eprint!("Accept this Session? Type YES: ");
        std::io::stderr().flush().context("flush approval prompt")?;
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .context("read approval response")?;
        Ok::<bool, anyhow::Error>(answer.trim() == "YES")
    })
    .await
    .context("join Linux approval prompt")?
}

#[cfg(target_os = "linux")]
const fn mark(enabled: bool) -> &'static str {
    if enabled { "x" } else { " " }
}

#[cfg(target_os = "linux")]
async fn run_heartbeats(
    client: reqwest::Client,
    control_url: String,
    device_id: DeviceId,
    identity: Arc<Ed25519DeviceIdentity>,
    nonce: Arc<AtomicU64>,
    capabilities: SessionPermissions,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let request = match signed_proof(&identity, "heartbeat", &device_id, &nonce) {
            Ok(proof) => DeviceHeartbeatRequest {
                proof,
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
                platform: DevicePlatform::Linux,
                capabilities,
                connectivity_candidates: Vec::new(),
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
            Ok(response) => warn!(event = "heartbeat_rejected", status = %response.status()),
            Err(error) => warn!(event = "heartbeat_failed", %error),
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
async fn report_event(
    client: &reqwest::Client,
    config: &Config,
    device_id: &DeviceId,
    identity: &Ed25519DeviceIdentity,
    nonce: &AtomicU64,
    session_id: SessionId,
    kind: SessionAuditEventKind,
    bytes_transferred: u64,
    result: &str,
) -> anyhow::Result<()> {
    let kind_text = match kind {
        SessionAuditEventKind::Started => "started",
        SessionAuditEventKind::Ended => "ended",
    };
    let action = format!("session_event:{session_id}:{kind_text}");
    client
        .post(format!(
            "{}/api/devices/{device_id}/sessions/{session_id}/events",
            config.control_url
        ))
        .json(&ReportSessionEventRequest {
            proof: signed_proof(identity, &action, device_id, nonce)?,
            kind,
            connection_type: ConnectionType::Relay,
            bytes_transferred,
            result: result.to_owned(),
        })
        .send()
        .await
        .context("report Linux Session event")?
        .error_for_status()
        .context("control server rejected Linux Session event")?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn run_session(config: &Config, credentials: SessionCredentials) -> anyhow::Result<u64> {
    if credentials.expires_at_ms <= now_ms()? {
        anyhow::bail!("Linux Session credentials expired");
    }
    let relay_address: SocketAddr = credentials
        .relay_address
        .parse()
        .context("parse Relay address")?;
    let session_id = credentials.session_id;
    let session_key = parse_key(&credentials.end_to_end_key_hex)?;
    let outbound_cipher = XChaChaSessionCipher::new(
        session_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let inbound_cipher = XChaChaSessionCipher::new(
        session_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::ControllerToAgent,
    );
    let endpoint = client_endpoint(&config.relay_certificate_path)?;
    let connection = endpoint
        .connect(relay_address, &credentials.relay_server_name)
        .context("create Relay connection")?
        .await
        .context("connect to Relay")?;
    let (send, receive) = connection.open_bi().await.context("open Relay stream")?;
    let mut transport = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
    transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::ClientHello(
            remotex_protocol::ClientHello::new(
                session_id,
                Role::Agent,
                parse_token(&credentials.role_token_hex)?,
            ),
        ))?))
        .await?;
    wait_for_peer(&mut transport).await?;
    let filesystem = if config.file_roots.is_empty() {
        None
    } else {
        Some(RootedFileSystem::new(config.file_roots.clone())?)
    };
    let mut capabilities = ActiveCapabilities {
        permissions: credentials.permissions.intersect(config.permissions),
        files: FileService {
            filesystem,
            upload_permission: credentials.permissions.file_upload,
            download_permission: credentials.permissions.file_download,
            uploads: TransferRegistry::default(),
            downloads: TransferRegistry::default(),
        },
        terminals: HashMap::new(),
    };
    let (terminal_sender, mut terminal_receiver) = mpsc::channel(TERMINAL_OUTPUT_QUEUE);
    let mut outbound_sequence = 0_u64;
    let mut expected_sequence = 0_u64;
    let mut bytes_transferred = 0_u64;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            terminal = terminal_receiver.recv() => {
                let Some(terminal) = terminal else { break; };
                bytes_transferred = bytes_transferred.saturating_add(send_message(
                    &mut transport, session_id, &outbound_cipher, &mut outbound_sequence,
                    Message::Terminal(terminal),
                ).await?);
            }
            incoming = transport.receive() => {
                let incoming = incoming?;
                bytes_transferred = bytes_transferred.saturating_add(
                    u64::try_from(incoming.len()).unwrap_or(u64::MAX)
                );
                for response in handle_relay(
                    &mut transport,
                    decode_wire(&incoming)?, session_id, &inbound_cipher,
                    &mut expected_sequence, &mut capabilities, &terminal_sender,
                ).await? {
                    bytes_transferred = bytes_transferred.saturating_add(send_message(
                        &mut transport, session_id, &outbound_cipher, &mut outbound_sequence, response,
                    ).await?);
                }
            }
        }
    }
    capabilities.terminals.clear();
    let _result = transport
        .send(Bytes::from(encode_wire(&RelayClientMessage::Close)?))
        .await;
    endpoint.close(0_u32.into(), b"Linux Agent Session ended");
    Ok(bytes_transferred)
}

#[cfg(target_os = "linux")]
async fn handle_relay(
    transport: &mut QuicFrameConnection,
    relay: RelayServerMessage,
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    expected_sequence: &mut u64,
    capabilities: &mut ActiveCapabilities,
    terminal_output: &mpsc::Sender<TerminalMessage>,
) -> anyhow::Result<Vec<Message>> {
    match relay {
        RelayServerMessage::Payload(payload) => {
            handle_controller_payload(
                &payload,
                session_id,
                cipher,
                expected_sequence,
                capabilities,
                terminal_output,
            )
            .await
        }
        RelayServerMessage::Heartbeat { nonce } => {
            transport
                .send(Bytes::from(encode_wire(
                    &RelayClientMessage::HeartbeatAck { nonce },
                )?))
                .await?;
            Ok(Vec::new())
        }
        RelayServerMessage::HeartbeatAck { .. }
        | RelayServerMessage::WaitingForPeer { .. }
        | RelayServerMessage::PeerReady => Ok(Vec::new()),
        RelayServerMessage::SessionClosed { reason } => {
            anyhow::bail!("Relay Session closed: {reason:?}")
        }
        RelayServerMessage::ProtocolError { code, message } => {
            anyhow::bail!("Relay protocol error {code:?}: {message}")
        }
    }
}

#[cfg(target_os = "linux")]
async fn handle_controller_payload(
    payload: &[u8],
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    expected_sequence: &mut u64,
    capabilities: &mut ActiveCapabilities,
    terminal_output: &mpsc::Sender<TerminalMessage>,
) -> anyhow::Result<Vec<Message>> {
    if payload.len() > MAX_FILE_CHUNK_SIZE as usize + 64 * 1024 || payload.len() < 8 {
        anyhow::bail!("Controller payload is outside Linux Agent limits");
    }
    let sequence = u64::from_be_bytes(payload[..8].try_into().context("read sequence")?);
    if sequence != *expected_sequence {
        anyhow::bail!(
            "unexpected Controller sequence {sequence}; expected {}",
            *expected_sequence
        );
    }
    let plaintext = cipher
        .open(sequence, &payload[8..])
        .context("authenticate Controller payload")?;
    let envelope: MessageEnvelope = decode_wire(&plaintext)?;
    envelope.validate()?;
    if envelope.session_id != session_id || envelope.sequence != sequence {
        anyhow::bail!("Controller envelope Session or sequence mismatch");
    }
    let responses = match envelope.message {
        Message::Terminal(message) => handle_terminal(capabilities, terminal_output, message)?,
        Message::System(SystemMessage::Request) if capabilities.permissions.system_info => {
            vec![Message::System(SystemMessage::Snapshot(
                tokio::task::spawn_blocking(system_snapshot)
                    .await
                    .context("collect Linux system information")?,
            ))]
        }
        Message::System(SystemMessage::Request) => vec![Message::System(SystemMessage::Error {
            message: "system information permission denied".to_owned(),
        })],
        Message::FileTransfer(message) => capabilities
            .files
            .handle(message)
            .await
            .into_iter()
            .map(Message::FileTransfer)
            .collect(),
        Message::Control(ControlMessage::VideoCapabilities { .. }) => Vec::new(),
        _ => anyhow::bail!("Controller sent a message not supported by the Linux Agent"),
    };
    *expected_sequence = expected_sequence
        .checked_add(1)
        .context("Controller sequence exhausted")?;
    Ok(responses)
}

#[cfg(target_os = "linux")]
fn handle_terminal(
    capabilities: &mut ActiveCapabilities,
    terminal_output: &mpsc::Sender<TerminalMessage>,
    message: TerminalMessage,
) -> anyhow::Result<Vec<Message>> {
    if !capabilities.permissions.terminal {
        return Ok(vec![Message::Terminal(TerminalMessage::Error {
            terminal_id: terminal_id(&message),
            message: "terminal permission denied".to_owned(),
        })]);
    }
    let response = match message {
        TerminalMessage::Open {
            terminal_id,
            columns,
            rows,
        } => {
            if capabilities.terminals.len() >= MAX_TERMINALS_PER_SESSION
                || capabilities.terminals.contains_key(&terminal_id)
            {
                Some(TerminalMessage::Error {
                    terminal_id: Some(terminal_id),
                    message: "terminal limit reached or ID already exists".to_owned(),
                })
            } else {
                let terminal =
                    TerminalSession::open(terminal_id, columns, rows, terminal_output.clone())?;
                capabilities.terminals.insert(terminal_id, terminal);
                None
            }
        }
        TerminalMessage::Input { terminal_id, data } => {
            if data.len() > MAX_TERMINAL_DATA_SIZE {
                anyhow::bail!("terminal input exceeds protocol limit");
            }
            capabilities
                .terminals
                .get_mut(&terminal_id)
                .context("terminal was not found")?
                .input(&data)?;
            None
        }
        TerminalMessage::Resize {
            terminal_id,
            columns,
            rows,
        } => {
            capabilities
                .terminals
                .get(&terminal_id)
                .context("terminal was not found")?
                .resize(columns, rows)?;
            None
        }
        TerminalMessage::Close { terminal_id } => {
            let exit_code = capabilities
                .terminals
                .remove(&terminal_id)
                .context("terminal was not found")?
                .close()?;
            Some(TerminalMessage::Closed {
                terminal_id,
                exit_code,
            })
        }
        TerminalMessage::Output { .. }
        | TerminalMessage::Closed { .. }
        | TerminalMessage::Error { .. } => {
            anyhow::bail!("Controller sent an Agent-only terminal message")
        }
    };
    Ok(response
        .map(|message| vec![Message::Terminal(message)])
        .unwrap_or_default())
}

#[cfg(target_os = "linux")]
const fn terminal_id(message: &TerminalMessage) -> Option<TerminalId> {
    match message {
        TerminalMessage::Open { terminal_id, .. }
        | TerminalMessage::Input { terminal_id, .. }
        | TerminalMessage::Resize { terminal_id, .. }
        | TerminalMessage::Output { terminal_id, .. }
        | TerminalMessage::Close { terminal_id }
        | TerminalMessage::Closed { terminal_id, .. } => Some(*terminal_id),
        TerminalMessage::Error { terminal_id, .. } => *terminal_id,
    }
}

#[cfg(target_os = "linux")]
async fn send_message(
    transport: &mut dyn Connection,
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    sequence: &mut u64,
    message: Message,
) -> anyhow::Result<u64> {
    let envelope = MessageEnvelope::new(session_id, *sequence, now_ms()?, message);
    let plaintext = encode_wire(&envelope)?;
    let ciphertext = cipher.seal(*sequence, &plaintext)?;
    let mut payload = Vec::with_capacity(8 + ciphertext.len());
    payload.extend_from_slice(&sequence.to_be_bytes());
    payload.extend_from_slice(&ciphertext);
    let encoded = encode_wire(&RelayClientMessage::Payload(payload))?;
    let bytes = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
    transport.send(Bytes::from(encoded)).await?;
    *sequence = sequence
        .checked_add(1)
        .context("Agent sequence exhausted")?;
    Ok(bytes)
}

#[cfg(target_os = "linux")]
impl FileService {
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
                match &self.filesystem {
                    Some(filesystem) => match filesystem.list_directory(&path).await {
                        Ok(entries) => vec![FileTransferMessage::ListDirectoryResponse {
                            request_id,
                            path,
                            entries,
                        }],
                        Err(error) => vec![file_service_error(Some(request_id), None, &error)],
                    },
                    None => vec![file_error(
                        Some(request_id),
                        None,
                        FileTransferErrorCode::PermissionDenied,
                    )],
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
                match &self.filesystem {
                    Some(filesystem) => match filesystem.create_directory(&path).await {
                        Ok(()) => {
                            vec![FileTransferMessage::CreateDirectoryResponse { request_id, path }]
                        }
                        Err(error) => vec![file_service_error(Some(request_id), None, &error)],
                    },
                    None => vec![file_error(
                        Some(request_id),
                        None,
                        FileTransferErrorCode::PermissionDenied,
                    )],
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
                match &self.filesystem {
                    Some(filesystem) => match filesystem.open_download(&source_path).await {
                        Ok(transfer) => {
                            let message = FileTransferMessage::Start {
                                transfer_id,
                                direction: FileTransferDirection::Download,
                                filename: transfer.filename().unwrap_or_default(),
                                source_path,
                                destination_path: String::new(),
                                total_size: transfer.total_size(),
                                chunk_size: transfer.chunk_size(),
                                sha256: transfer.sha256(),
                            };
                            match self.downloads.insert(transfer_id, transfer) {
                                Ok(()) => vec![message],
                                Err(error) => {
                                    vec![file_service_error(None, Some(transfer_id), &error)]
                                }
                            }
                        }
                        Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                    },
                    None => vec![file_error(
                        None,
                        Some(transfer_id),
                        FileTransferErrorCode::PermissionDenied,
                    )],
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
                match &self.filesystem {
                    Some(filesystem) => match filesystem
                        .open_upload(
                            transfer_id,
                            &destination_path,
                            total_size,
                            chunk_size,
                            sha256,
                        )
                        .await
                    {
                        Ok(transfer) => {
                            let next_offset = transfer.next_offset();
                            match self.uploads.insert(transfer_id, transfer) {
                                Ok(()) => vec![FileTransferMessage::Accept {
                                    transfer_id,
                                    next_offset,
                                }],
                                Err(error) => {
                                    vec![file_service_error(None, Some(transfer_id), &error)]
                                }
                            }
                        }
                        Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                    },
                    None => vec![file_error(
                        None,
                        Some(transfer_id),
                        FileTransferErrorCode::PermissionDenied,
                    )],
                }
            }
            FileTransferMessage::Chunk {
                transfer_id,
                offset,
                checksum,
                payload,
            } => match self.uploads.get_mut(&transfer_id) {
                Ok(transfer) => match transfer.write_chunk(offset, checksum, &payload).await {
                    Ok(next_offset) => vec![FileTransferMessage::ChunkAck {
                        transfer_id,
                        next_offset,
                    }],
                    Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                },
                Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
            },
            FileTransferMessage::Complete {
                transfer_id,
                total_size,
                sha256,
            } => match self.uploads.remove(&transfer_id) {
                Ok(transfer) => match transfer.complete(total_size, sha256).await {
                    Ok(()) => vec![FileTransferMessage::Progress {
                        transfer_id,
                        next_offset: total_size,
                    }],
                    Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
                },
                Err(error) => vec![file_service_error(None, Some(transfer_id), &error)],
            },
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
            } => vec![self.download_chunk(transfer_id, next_offset).await],
            FileTransferMessage::Cancel { transfer_id } => {
                if self.uploads.contains(&transfer_id) {
                    match self.uploads.remove(&transfer_id) {
                        Ok(transfer) => {
                            let _result = transfer.cancel().await;
                        }
                        Err(error) => {
                            return vec![file_service_error(None, Some(transfer_id), &error)];
                        }
                    }
                }
                if self.downloads.contains(&transfer_id) {
                    let _result = self.downloads.remove(&transfer_id);
                }
                vec![FileTransferMessage::Progress {
                    transfer_id,
                    next_offset: 0,
                }]
            }
            FileTransferMessage::Error {
                transfer_id: Some(transfer_id),
                ..
            } => {
                if self.uploads.contains(&transfer_id) {
                    let _result = self.uploads.remove(&transfer_id);
                }
                if self.downloads.contains(&transfer_id) {
                    let _result = self.downloads.remove(&transfer_id);
                }
                Vec::new()
            }
            FileTransferMessage::Start { transfer_id, .. }
            | FileTransferMessage::Progress { transfer_id, .. } => vec![file_error(
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

#[cfg(target_os = "linux")]
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
        FileTransferError::TooManyTransfers => FileTransferErrorCode::Busy,
        FileTransferError::Io(_) => FileTransferErrorCode::Io,
        _ => FileTransferErrorCode::InvalidPath,
    };
    file_error(request_id, transfer_id, code)
}

#[cfg(target_os = "linux")]
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
        FileTransferErrorCode::Io => "file I/O failed",
    };
    FileTransferMessage::Error {
        request_id,
        transfer_id,
        code,
        message: message.to_owned(),
    }
}

#[cfg(target_os = "linux")]
async fn wait_for_peer(transport: &mut QuicFrameConnection) -> anyhow::Result<()> {
    loop {
        match decode_wire::<RelayServerMessage>(&transport.receive().await?)? {
            RelayServerMessage::WaitingForPeer { role } => info!(?role, "waiting for Relay peer"),
            RelayServerMessage::PeerReady => return Ok(()),
            RelayServerMessage::Heartbeat { nonce } => {
                transport
                    .send(Bytes::from(encode_wire(
                        &RelayClientMessage::HeartbeatAck { nonce },
                    )?))
                    .await?;
            }
            RelayServerMessage::HeartbeatAck { .. } => {}
            RelayServerMessage::SessionClosed { reason } => {
                anyhow::bail!("Relay Session closed before pairing: {reason:?}")
            }
            RelayServerMessage::ProtocolError { code, message } => {
                anyhow::bail!("Relay protocol error {code:?}: {message}")
            }
            RelayServerMessage::Payload(_) => {
                anyhow::bail!("Relay delivered payload before PeerReady")
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn load_config() -> anyhow::Result<Config> {
    let permissions = SessionPermissions {
        view_desktop: false,
        control_input: false,
        clipboard: false,
        file_upload: parse_switch("REMOTEX_ALLOW_FILE_UPLOAD", false)?,
        file_download: parse_switch("REMOTEX_ALLOW_FILE_DOWNLOAD", false)?,
        terminal: parse_switch("REMOTEX_ALLOW_TERMINAL", false)?,
        system_info: parse_switch("REMOTEX_ALLOW_SYSTEM_INFO", true)?,
    };
    let file_roots = parse_file_roots(std::env::var("REMOTEX_FILE_ROOTS").ok().as_deref())?;
    if (permissions.file_upload || permissions.file_download) && file_roots.is_empty() {
        anyhow::bail!("REMOTEX_FILE_ROOTS is required when Linux file access is enabled");
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
    Ok(Config {
        control_url: required("REMOTEX_CONTROL_URL")?
            .trim_end_matches('/')
            .to_owned(),
        identity_path: PathBuf::from(required("REMOTEX_IDENTITY_PATH")?),
        relay_certificate_path: PathBuf::from(required("REMOTEX_RELAY_CA_CERT")?),
        device_name: std::env::var("REMOTEX_DEVICE_NAME")
            .unwrap_or_else(|_| "Linux Server".to_owned()),
        permissions,
        unattended_access,
        unattended_secret,
        file_roots,
    })
}

#[cfg(target_os = "linux")]
fn parse_file_roots(value: Option<&str>) -> anyhow::Result<Vec<AllowedRoot>> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    value
        .split(';')
        .map(|entry| {
            let (name, path) = entry
                .split_once('=')
                .context("REMOTEX_FILE_ROOTS entries must use Name=/absolute/path")?;
            if name.trim().is_empty() || path.trim().is_empty() {
                anyhow::bail!("file root names and paths must not be empty");
            }
            Ok(AllowedRoot {
                name: name.trim().to_owned(),
                path: PathBuf::from(path.trim()),
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn load_or_create_identity(path: &Path) -> anyhow::Result<Ed25519DeviceIdentity> {
    match File::open(path) {
        Ok(file) => {
            let metadata = file
                .metadata()
                .with_context(|| format!("inspect Linux identity {}", path.display()))?;
            anyhow::ensure!(metadata.len() <= 64 * 1024, "Linux identity exceeds 64 KiB");
            let capacity = usize::try_from(metadata.len()).context("identity size is invalid")?;
            let mut bytes = Vec::with_capacity(capacity);
            file.take(64 * 1024 + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("read Linux identity {}", path.display()))?;
            anyhow::ensure!(bytes.len() <= 64 * 1024, "Linux identity exceeds 64 KiB");
            let stored: StoredIdentity = serde_json::from_slice(&bytes)?;
            let mut secret = parse_key(&stored.secret_key_hex)?;
            let identity = Ed25519DeviceIdentity::from_secret_bytes(secret);
            secret.fill(0);
            Ok(identity)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create identity directory {}", parent.display()))?;
            }
            let secret: [u8; 32] = rand::random();
            let identity = Ed25519DeviceIdentity::from_secret_bytes(secret);
            let bytes = serde_json::to_vec_pretty(&StoredIdentity {
                secret_key_hex: hex::encode(secret),
            })?;
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            options.mode(0o600);
            let mut file = options
                .open(path)
                .with_context(|| format!("create identity {}", path.display()))?;
            file.write_all(&bytes).context("write identity")?;
            Ok(identity)
        }
        Err(error) => Err(error).context("read Linux identity"),
    }
}

#[cfg(target_os = "linux")]
fn signed_proof(
    identity: &Ed25519DeviceIdentity,
    action: &str,
    device_id: &DeviceId,
    nonce: &AtomicU64,
) -> anyhow::Result<DeviceAuthProof> {
    let timestamp_ms = now_ms()?;
    let nonce = nonce
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |previous| {
            Some(timestamp_ms.max(previous.saturating_add(1)))
        })
        .map_err(|_| anyhow::anyhow!("device proof nonce update failed"))?
        .saturating_add(1);
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

#[cfg(target_os = "linux")]
fn client_endpoint(certificate_path: &Path) -> anyhow::Result<Endpoint> {
    let mut reader = BufReader::new(
        File::open(certificate_path)
            .with_context(|| format!("open Relay CA {}", certificate_path.display()))?,
    );
    let certificates = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .context("read Relay CA")?;
    if certificates.is_empty() {
        anyhow::bail!("Relay CA file contains no certificates");
    }
    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate).context("trust Relay CA")?;
    }
    let config = ClientConfig::with_root_certificates(Arc::new(roots))?;
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

#[cfg(target_os = "linux")]
fn parse_token(value: &str) -> anyhow::Result<SessionToken> {
    let bytes: [u8; 32] = hex::decode(value)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Session token must contain exactly 32 bytes"))?;
    Ok(SessionToken::from_bytes(bytes))
}

#[cfg(target_os = "linux")]
fn parse_key(value: &str) -> anyhow::Result<[u8; 32]> {
    hex::decode(value)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("key must contain exactly 32 bytes"))
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

#[cfg(target_os = "linux")]
fn now_ms() -> anyhow::Result<u64> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    u64::try_from(duration.as_millis()).context("system timestamp is out of range")
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("remotex-linux-agent is supported on Linux only");
}
