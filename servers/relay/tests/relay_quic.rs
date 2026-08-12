use quinn::{ClientConfig, Endpoint, ServerConfig};
use rcgen::generate_simple_self_signed;
use remotex_capture::{Frame, PixelFormat};
use remotex_crypto::{SessionCipher, SessionDirection, XChaChaSessionCipher};
use remotex_protocol::{Message, MessageEnvelope, RelayHandshake, Role, SessionId, SessionToken};
use remotex_relay::{RelayLimits, RelayServer, SessionAuthorizer};
use remotex_transport::{read_frame, write_frame};
use remotex_video::{EncoderConfig, SoftwareEncoder, decode};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::{error::Error, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::oneshot;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn endpoints() -> TestResult<(Endpoint, Endpoint, SocketAddr)> {
    let generated = generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let certificate: CertificateDer<'static> = generated.cert.der().clone();
    let key = PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der());
    let server_config = ServerConfig::with_single_cert(vec![certificate.clone()], key.into())?;
    let server = Endpoint::server(server_config, "127.0.0.1:0".parse()?)?;
    let server_address = server.local_addr()?;

    let mut roots = RootCertStore::empty();
    roots.add(certificate)?;
    let client_config = ClientConfig::with_root_certificates(Arc::new(roots))?;
    let mut client = Endpoint::client("0.0.0.0:0".parse()?)?;
    client.set_default_client_config(client_config);
    Ok((server, client, server_address))
}

async fn connect_peer(
    endpoint: &Endpoint,
    server_address: SocketAddr,
    handshake: &RelayHandshake,
) -> TestResult<(quinn::SendStream, quinn::RecvStream)> {
    let connection = endpoint.connect(server_address, "localhost")?.await?;
    let (mut send, receive) = connection.open_bi().await?;
    let encoded = bincode::serde::encode_to_vec(handshake, bincode::config::standard())?;
    write_frame(&mut send, &encoded, 1024).await?;
    Ok((send, receive))
}

#[tokio::test]
async fn mock_controller_and_agent_exchange_binary_frames_through_quic() -> TestResult {
    let (server_endpoint, client_endpoint, server_address) = endpoints()?;
    let authorizer = SessionAuthorizer::default();
    let session_id = SessionId::new();
    let controller_token = SessionToken::from_bytes([1; 32]);
    let agent_token = SessionToken::from_bytes([2; 32]);
    authorizer
        .grant(
            session_id,
            Role::Controller,
            controller_token.clone(),
            u64::MAX,
        )
        .await;
    authorizer
        .grant(session_id, Role::Agent, agent_token.clone(), u64::MAX)
        .await;

    let relay = RelayServer::new(
        authorizer,
        RelayLimits {
            maximum_frame_size: 1024 * 1024,
            handshake_timeout: Duration::from_secs(2),
            idle_timeout: Duration::from_secs(2),
        },
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server_task = tokio::spawn(async move {
        relay
            .serve_until(server_endpoint, async {
                let _result = shutdown_rx.await;
            })
            .await
    });

    let controller_handshake = RelayHandshake::new(session_id, Role::Controller, controller_token);
    let agent_handshake = RelayHandshake::new(session_id, Role::Agent, agent_token);
    let (mut controller_send, mut controller_receive) =
        connect_peer(&client_endpoint, server_address, &controller_handshake).await?;
    let (mut agent_send, mut agent_receive) =
        connect_peer(&client_endpoint, server_address, &agent_handshake).await?;

    write_frame(&mut controller_send, b"controller-to-agent", 1024).await?;
    assert_eq!(
        read_frame(&mut agent_receive, 1024).await?,
        &b"controller-to-agent"[..]
    );
    write_frame(&mut agent_send, b"agent-to-controller", 1024).await?;
    assert_eq!(
        read_frame(&mut controller_receive, 1024).await?,
        &b"agent-to-controller"[..]
    );

    let capture = Frame {
        width: 64,
        height: 36,
        stride: 64 * 4,
        pixel_format: PixelFormat::Bgra8,
        timestamp_ms: 1234,
        data: vec![64; 64 * 36 * 4],
    };
    let video = SoftwareEncoder::new(EncoderConfig::default())?.encode(&capture)?;
    let envelope = MessageEnvelope::new(session_id, 7, capture.timestamp_ms, Message::Video(video));
    let encoded_video = bincode::serde::encode_to_vec(&envelope, bincode::config::standard())?;
    let cipher = XChaChaSessionCipher::new(
        [19; 32],
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let encrypted_video = cipher.seal(envelope.sequence, &encoded_video)?;
    let mut wire_frame = envelope.sequence.to_be_bytes().to_vec();
    wire_frame.extend_from_slice(&encrypted_video);
    write_frame(&mut agent_send, &wire_frame, 1024 * 1024).await?;
    let relayed_video = read_frame(&mut controller_receive, 1024 * 1024).await?;
    assert_ne!(relayed_video, encoded_video);
    let sequence = u64::from_be_bytes(relayed_video[..8].try_into()?);
    let decrypted_video = cipher.open(sequence, &relayed_video[8..])?;
    let (received, consumed): (MessageEnvelope, usize) =
        bincode::serde::decode_from_slice(&decrypted_video, bincode::config::standard())?;
    assert_eq!(consumed, decrypted_video.len());
    received.validate()?;
    let Message::Video(received_video) = received.message else {
        return Err("expected a video message".into());
    };
    let decoded = decode(&received_video)?;
    assert_eq!((decoded.width, decoded.height), (64, 36));
    assert_eq!(decoded.rgba.len(), 64 * 36 * 4);

    client_endpoint.close(0_u32.into(), b"test complete");
    let _ = shutdown_tx.send(());
    server_task.await??;
    Ok(())
}
