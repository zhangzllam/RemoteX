//! `RemoteX` M3 Tauri controller and remote-video display bridge.

use anyhow::Context;
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use quinn::{ClientConfig, Endpoint};
use remotex_crypto::{SessionCipher, SessionDirection, XChaChaSessionCipher};
use remotex_protocol::{
    Message, MessageEnvelope, RelayHandshake, Role, SessionId, SessionToken, VideoCodec,
};
use remotex_transport::{Connection, DEFAULT_MAX_FRAME_SIZE, QuicFrameConnection};
use rustls::RootCertStore;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::BufReader, net::SocketAddr, path::Path, sync::Arc};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;

#[derive(Default)]
struct StreamControl {
    cancellation: std::sync::Mutex<Option<oneshot::Sender<()>>>,
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

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn connect_remote(
    app: AppHandle,
    control: State<'_, StreamControl>,
    request: ConnectRequest,
) -> Result<(), String> {
    let mut cancellation = control
        .cancellation
        .lock()
        .map_err(|_| "stream control lock is unavailable".to_owned())?;
    if let Some(previous) = cancellation.take() {
        let _result = previous.send(());
    }
    let (cancel_sender, cancel_receiver) = oneshot::channel();
    *cancellation = Some(cancel_sender);
    drop(cancellation);

    tauri::async_runtime::spawn(async move {
        emit_status(&app, "connecting", "Connecting to relay");
        let result = receive_video(app.clone(), request, cancel_receiver).await;
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
    let mut cancellation = control
        .cancellation
        .lock()
        .map_err(|_| "stream control lock is unavailable".to_owned())?;
    if let Some(sender) = cancellation.take() {
        let _result = sender.send(());
    }
    emit_status(&app, "disconnected", "Disconnected by user");
    Ok(())
}

async fn receive_video(
    app: AppHandle,
    request: ConnectRequest,
    mut cancellation: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let relay_address: SocketAddr = request
        .relay_address
        .parse()
        .context("parse relay address")?;
    let session_id: SessionId = request.session_id.parse().context("parse session ID")?;
    let token = parse_token(&request.token_hex)?;
    let end_to_end_key = parse_key(&request.end_to_end_key_hex)?;
    let cipher = XChaChaSessionCipher::new(
        end_to_end_key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let endpoint = client_endpoint(Path::new(&request.ca_certificate_path))?;
    let connection = endpoint
        .connect(relay_address, &request.server_name)
        .context("create relay connection")?
        .await
        .context("connect to relay")?;
    let (send, receive) = connection.open_bi().await.context("open relay stream")?;
    let mut transport = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
    let handshake = RelayHandshake::new(session_id, Role::Controller, token);
    let encoded_handshake = bincode::serde::encode_to_vec(&handshake, bincode::config::standard())
        .context("encode relay handshake")?;
    transport.send(Bytes::from(encoded_handshake)).await?;
    emit_status(&app, "connected", "Waiting for remote video");

    loop {
        let bytes = tokio::select! {
            _ = &mut cancellation => break,
            result = transport.receive() => result?,
        };
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
        let (envelope, consumed): (MessageEnvelope, usize) =
            bincode::serde::decode_from_slice(&plaintext, bincode::config::standard())
                .context("decode protocol envelope")?;
        if consumed != plaintext.len() {
            anyhow::bail!("video envelope contains trailing bytes");
        }
        envelope.validate()?;
        if envelope.session_id != session_id {
            anyhow::bail!("received a frame for a different session");
        }
        if envelope.sequence != sequence {
            anyhow::bail!("encrypted frame sequence does not match its envelope");
        }
        let Message::Video(frame) = envelope.message else {
            continue;
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
    }
    endpoint.close(0_u32.into(), b"controller disconnected");
    Ok(())
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
        .invoke_handler(tauri::generate_handler![connect_remote, disconnect_remote])
        .run(tauri::generate_context!())
        .expect("run RemoteX desktop application");
}
