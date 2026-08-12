use quinn::{ClientConfig, Endpoint, ServerConfig};
use rcgen::generate_simple_self_signed;
use remotex_capture::{Frame, PixelFormat};
use remotex_crypto::{SessionCipher, SessionDirection, XChaChaSessionCipher};
use remotex_protocol::{
    ClientHello, ClipboardMessage, ClipboardOrigin, FileTransferDirection, FileTransferMessage,
    InputEvent, KeyCode, Message, MessageEnvelope, MouseButton, RelayClientMessage,
    RelayProtocolErrorCode, RelayServerMessage, Role, SessionCloseReason, SessionId, SessionToken,
    TransferId, decode_wire, encode_wire,
};
use remotex_relay::{InMemorySessionAuthenticator, RelayLimits, RelayServer};
use remotex_transport::{read_frame, write_frame};
use remotex_video::{EncoderConfig, SoftwareEncoder, decode};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::{error::Error, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    sync::oneshot,
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

struct Harness {
    client_endpoint: Endpoint,
    server_address: SocketAddr,
    authenticator: InMemorySessionAuthenticator,
    relay: RelayServer,
    shutdown: oneshot::Sender<()>,
    server_task: JoinHandle<Result<(), remotex_relay::RelayError>>,
}

impl Harness {
    fn limits() -> RelayLimits {
        RelayLimits {
            maximum_frame_size: 4 * 1024,
            max_connections: 32,
            max_pending_sessions: 16,
            outbound_queue_capacity: 8,
            handshake_timeout: Duration::from_secs(1),
            heartbeat_interval: Duration::from_millis(500),
            peer_timeout: Duration::from_secs(2),
        }
    }

    fn start(limits: RelayLimits) -> TestResult<Self> {
        let generated = generate_simple_self_signed(vec!["localhost".to_owned()])?;
        let certificate: CertificateDer<'static> = generated.cert.der().clone();
        let key = PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der());
        let server_config = ServerConfig::with_single_cert(vec![certificate.clone()], key.into())?;
        let server_endpoint = Endpoint::server(server_config, "127.0.0.1:0".parse()?)?;
        let server_address = server_endpoint.local_addr()?;

        let mut roots = RootCertStore::empty();
        roots.add(certificate)?;
        let client_config = ClientConfig::with_root_certificates(Arc::new(roots))?;
        let mut client_endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
        client_endpoint.set_default_client_config(client_config);

        let authenticator = InMemorySessionAuthenticator::default();
        let relay = RelayServer::new(authenticator.clone(), limits);
        let server_relay = relay.clone();
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            server_relay
                .serve_until(server_endpoint, async {
                    let _result = shutdown_receiver.await;
                })
                .await
        });
        Ok(Self {
            client_endpoint,
            server_address,
            authenticator,
            relay,
            shutdown,
            server_task,
        })
    }

    async fn grant_pair(
        &self,
        session_id: SessionId,
        controller_token: SessionToken,
        agent_token: SessionToken,
    ) {
        self.authenticator
            .grant(session_id, Role::Controller, controller_token, u64::MAX)
            .await;
        self.authenticator
            .grant(session_id, Role::Agent, agent_token, u64::MAX)
            .await;
    }

    async fn connect(&self, hello: ClientHello, maximum_frame_size: usize) -> TestResult<TestPeer> {
        let connection = self
            .client_endpoint
            .connect(self.server_address, "localhost")?
            .await?;
        let (mut send, receive) = connection.open_bi().await?;
        let message = RelayClientMessage::ClientHello(hello);
        write_frame(&mut send, &encode_wire(&message)?, maximum_frame_size).await?;
        Ok(TestPeer {
            connection,
            send,
            receive,
            maximum_frame_size,
        })
    }

    async fn pair(
        &self,
        session_id: SessionId,
        controller_token: SessionToken,
        agent_token: SessionToken,
    ) -> TestResult<(TestPeer, TestPeer)> {
        self.grant_pair(session_id, controller_token.clone(), agent_token.clone())
            .await;
        let mut controller = self
            .connect(
                ClientHello::new(session_id, Role::Controller, controller_token),
                self.relay.limits().maximum_frame_size,
            )
            .await?;
        assert_eq!(
            controller.receive_significant().await?,
            RelayServerMessage::WaitingForPeer { role: Role::Agent }
        );
        let mut agent = self
            .connect(
                ClientHello::new(session_id, Role::Agent, agent_token),
                self.relay.limits().maximum_frame_size,
            )
            .await?;
        assert_eq!(
            agent.receive_significant().await?,
            RelayServerMessage::PeerReady
        );
        assert_eq!(
            controller.receive_significant().await?,
            RelayServerMessage::PeerReady
        );
        Ok((controller, agent))
    }

    async fn wait_for_empty_registry(&self) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if self.relay.active_session_count().await == 0
                && self.relay.connected_peer_count().await == 0
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("relay registry did not clean up before the deadline".into());
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    async fn stop(self) -> TestResult {
        let _result = self.shutdown.send(());
        self.client_endpoint
            .close(0_u32.into(), b"integration test complete");
        self.server_task.await??;
        Ok(())
    }
}

struct TestPeer {
    connection: quinn::Connection,
    send: quinn::SendStream,
    receive: quinn::RecvStream,
    maximum_frame_size: usize,
}

impl TestPeer {
    async fn send(&mut self, message: &RelayClientMessage) -> TestResult {
        write_frame(
            &mut self.send,
            &encode_wire(message)?,
            self.maximum_frame_size,
        )
        .await?;
        Ok(())
    }

    async fn receive(&mut self) -> TestResult<RelayServerMessage> {
        let bytes = timeout(
            Duration::from_secs(2),
            read_frame(&mut self.receive, self.maximum_frame_size),
        )
        .await??;
        Ok(decode_wire(&bytes)?)
    }

    async fn receive_significant(&mut self) -> TestResult<RelayServerMessage> {
        loop {
            let message = self.receive().await?;
            if let RelayServerMessage::Heartbeat { nonce } = message {
                self.send(&RelayClientMessage::HeartbeatAck { nonce })
                    .await?;
            } else {
                return Ok(message);
            }
        }
    }

    fn disconnect(&self) {
        self.connection
            .close(0_u32.into(), b"test peer disconnected");
    }
}

#[tokio::test]
async fn successful_pairing_notifies_both_peers() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    let (_controller, _agent) = harness
        .pair(
            session_id,
            SessionToken::from_bytes([1; 32]),
            SessionToken::from_bytes([2; 32]),
        )
        .await?;

    assert_eq!(harness.relay.active_session_count().await, 1);
    assert_eq!(harness.relay.connected_peer_count().await, 2);
    harness.stop().await
}

#[tokio::test]
async fn controller_payload_reaches_agent() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let (mut controller, mut agent) = harness
        .pair(
            SessionId::new(),
            SessionToken::from_bytes([3; 32]),
            SessionToken::from_bytes([4; 32]),
        )
        .await?;

    controller
        .send(&RelayClientMessage::Payload(b"message A".to_vec()))
        .await?;
    assert_eq!(
        agent.receive_significant().await?,
        RelayServerMessage::Payload(b"message A".to_vec())
    );
    harness.stop().await
}

#[tokio::test]
async fn agent_payload_reaches_controller() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let (mut controller, mut agent) = harness
        .pair(
            SessionId::new(),
            SessionToken::from_bytes([5; 32]),
            SessionToken::from_bytes([6; 32]),
        )
        .await?;

    agent
        .send(&RelayClientMessage::Payload(b"message B".to_vec()))
        .await?;
    assert_eq!(
        controller.receive_significant().await?,
        RelayServerMessage::Payload(b"message B".to_vec())
    );
    harness.stop().await
}

#[tokio::test]
async fn invalid_token_is_rejected() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    harness
        .authenticator
        .grant(
            session_id,
            Role::Controller,
            SessionToken::from_bytes([7; 32]),
            u64::MAX,
        )
        .await;
    let mut peer = harness
        .connect(
            ClientHello::new(
                session_id,
                Role::Controller,
                SessionToken::from_bytes([8; 32]),
            ),
            harness.relay.limits().maximum_frame_size,
        )
        .await?;

    assert!(matches!(
        peer.receive().await?,
        RelayServerMessage::ProtocolError {
            code: RelayProtocolErrorCode::InvalidToken,
            ..
        }
    ));
    harness.stop().await
}

#[tokio::test]
async fn token_for_the_opposite_role_is_rejected() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    let controller_token = SessionToken::from_bytes([9; 32]);
    harness
        .grant_pair(
            session_id,
            controller_token.clone(),
            SessionToken::from_bytes([10; 32]),
        )
        .await;
    let mut peer = harness
        .connect(
            ClientHello::new(session_id, Role::Agent, controller_token),
            harness.relay.limits().maximum_frame_size,
        )
        .await?;

    assert!(matches!(
        peer.receive().await?,
        RelayServerMessage::ProtocolError {
            code: RelayProtocolErrorCode::RoleMismatch,
            ..
        }
    ));
    harness.stop().await
}

#[tokio::test]
async fn duplicate_role_is_rejected() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    let first_token = SessionToken::from_bytes([11; 32]);
    harness
        .authenticator
        .grant(session_id, Role::Controller, first_token.clone(), u64::MAX)
        .await;
    let mut first = harness
        .connect(
            ClientHello::new(session_id, Role::Controller, first_token),
            harness.relay.limits().maximum_frame_size,
        )
        .await?;
    assert!(matches!(
        first.receive_significant().await?,
        RelayServerMessage::WaitingForPeer { .. }
    ));

    let second_token = SessionToken::from_bytes([12; 32]);
    harness
        .authenticator
        .grant(session_id, Role::Controller, second_token.clone(), u64::MAX)
        .await;
    let mut second = harness
        .connect(
            ClientHello::new(session_id, Role::Controller, second_token),
            harness.relay.limits().maximum_frame_size,
        )
        .await?;
    assert!(matches!(
        second.receive().await?,
        RelayServerMessage::ProtocolError {
            code: RelayProtocolErrorCode::DuplicateRole,
            ..
        }
    ));
    harness.stop().await
}

#[tokio::test]
async fn expired_session_is_rejected() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    let token = SessionToken::from_bytes([13; 32]);
    harness
        .authenticator
        .grant(session_id, Role::Agent, token.clone(), 0)
        .await;
    let mut peer = harness
        .connect(
            ClientHello::new(session_id, Role::Agent, token),
            harness.relay.limits().maximum_frame_size,
        )
        .await?;

    assert!(matches!(
        peer.receive().await?,
        RelayServerMessage::ProtocolError {
            code: RelayProtocolErrorCode::ExpiredSession,
            ..
        }
    ));
    harness.stop().await
}

#[tokio::test]
async fn oversized_frame_is_rejected_without_crashing_relay() -> TestResult {
    let limits = RelayLimits {
        maximum_frame_size: 256,
        ..Harness::limits()
    };
    let harness = Harness::start(limits)?;
    let session_id = SessionId::new();
    let token = SessionToken::from_bytes([14; 32]);
    harness
        .authenticator
        .grant(session_id, Role::Controller, token.clone(), u64::MAX)
        .await;
    let mut peer = harness
        .connect(
            ClientHello::new(session_id, Role::Controller, token),
            limits.maximum_frame_size,
        )
        .await?;
    assert!(matches!(
        peer.receive_significant().await?,
        RelayServerMessage::WaitingForPeer { .. }
    ));
    peer.send
        .write_u32(u32::try_from(limits.maximum_frame_size + 1)?)
        .await?;
    peer.send.flush().await?;

    assert!(matches!(
        peer.receive_significant().await?,
        RelayServerMessage::ProtocolError {
            code: RelayProtocolErrorCode::FrameTooLarge,
            ..
        }
    ));
    harness.wait_for_empty_registry().await?;
    harness.stop().await
}

#[tokio::test]
async fn peer_disconnect_notifies_other_peer_and_cleans_session() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let (mut controller, agent) = harness
        .pair(
            SessionId::new(),
            SessionToken::from_bytes([15; 32]),
            SessionToken::from_bytes([16; 32]),
        )
        .await?;
    agent.disconnect();

    assert_eq!(
        controller.receive_significant().await?,
        RelayServerMessage::SessionClosed {
            reason: SessionCloseReason::PeerDisconnected
        }
    );
    harness.wait_for_empty_registry().await?;
    harness.stop().await
}

#[tokio::test]
async fn heartbeat_timeout_removes_inactive_peer() -> TestResult {
    let limits = RelayLimits {
        heartbeat_interval: Duration::from_millis(20),
        peer_timeout: Duration::from_millis(100),
        ..Harness::limits()
    };
    let harness = Harness::start(limits)?;
    let session_id = SessionId::new();
    let token = SessionToken::from_bytes([17; 32]);
    harness
        .authenticator
        .grant(session_id, Role::Controller, token.clone(), u64::MAX)
        .await;
    let mut peer = harness
        .connect(
            ClientHello::new(session_id, Role::Controller, token),
            limits.maximum_frame_size,
        )
        .await?;
    assert!(matches!(
        peer.receive().await?,
        RelayServerMessage::WaitingForPeer { .. }
    ));
    sleep(Duration::from_millis(150)).await;

    let mut timed_out = false;
    while let Ok(Ok(message)) = timeout(Duration::from_millis(200), peer.receive()).await {
        if message
            == (RelayServerMessage::SessionClosed {
                reason: SessionCloseReason::HeartbeatTimeout,
            })
        {
            timed_out = true;
            break;
        }
    }
    assert!(timed_out, "inactive peer did not receive timeout closure");
    harness.wait_for_empty_registry().await?;
    harness.stop().await
}

#[tokio::test]
async fn encrypted_video_crosses_relay_as_opaque_payload() -> TestResult {
    let limits = RelayLimits {
        maximum_frame_size: 1024 * 1024,
        ..Harness::limits()
    };
    let harness = Harness::start(limits)?;
    let session_id = SessionId::new();
    let (mut controller, mut agent) = harness
        .pair(
            session_id,
            SessionToken::from_bytes([18; 32]),
            SessionToken::from_bytes([19; 32]),
        )
        .await?;
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
    let encoded_video = encode_wire(&envelope)?;
    let cipher = XChaChaSessionCipher::new(
        [20; 32],
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );
    let encrypted_video = cipher.seal(envelope.sequence, &encoded_video)?;
    let mut wire_payload = envelope.sequence.to_be_bytes().to_vec();
    wire_payload.extend_from_slice(&encrypted_video);
    agent
        .send(&RelayClientMessage::Payload(wire_payload.clone()))
        .await?;
    let RelayServerMessage::Payload(relayed_video) = controller.receive_significant().await? else {
        return Err("expected relayed video payload".into());
    };
    assert_eq!(relayed_video, wire_payload);
    assert_ne!(relayed_video, encoded_video);
    let sequence = u64::from_be_bytes(relayed_video[..8].try_into()?);
    let decrypted_video = cipher.open(sequence, &relayed_video[8..])?;
    let received: MessageEnvelope = decode_wire(&decrypted_video)?;
    received.validate()?;
    let Message::Video(received_video) = received.message else {
        return Err("expected a video message".into());
    };
    let decoded = decode(&received_video)?;
    assert_eq!((decoded.width, decoded.height), (64, 36));
    assert_eq!(decoded.rgba.len(), 64 * 36 * 4);
    harness.stop().await
}

#[tokio::test]
async fn encrypted_mouse_input_crosses_relay_as_opaque_payload() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    let (mut controller, mut agent) = harness
        .pair(
            session_id,
            SessionToken::from_bytes([21; 32]),
            SessionToken::from_bytes([22; 32]),
        )
        .await?;
    let envelope = MessageEnvelope::new(
        session_id,
        0,
        1_234,
        Message::Input(InputEvent::MouseButtonDown {
            button: MouseButton::Left,
        }),
    );
    let encoded_input = encode_wire(&envelope)?;
    let cipher = XChaChaSessionCipher::new(
        [23; 32],
        *session_id.as_uuid().as_bytes(),
        SessionDirection::ControllerToAgent,
    );
    let encrypted_input = cipher.seal(envelope.sequence, &encoded_input)?;
    let mut wire_payload = envelope.sequence.to_be_bytes().to_vec();
    wire_payload.extend_from_slice(&encrypted_input);

    controller
        .send(&RelayClientMessage::Payload(wire_payload.clone()))
        .await?;
    let RelayServerMessage::Payload(relayed_input) = agent.receive_significant().await? else {
        return Err("expected relayed input payload".into());
    };
    assert_eq!(relayed_input, wire_payload);
    assert_ne!(relayed_input, encoded_input);
    let sequence = u64::from_be_bytes(relayed_input[..8].try_into()?);
    let plaintext = cipher.open(sequence, &relayed_input[8..])?;
    let received: MessageEnvelope = decode_wire(&plaintext)?;
    received.validate()?;
    assert_eq!(received, envelope);
    harness.stop().await
}

#[tokio::test]
async fn encrypted_keyboard_combination_crosses_relay_in_order() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    let (mut controller, mut agent) = harness
        .pair(
            session_id,
            SessionToken::from_bytes([24; 32]),
            SessionToken::from_bytes([25; 32]),
        )
        .await?;
    let cipher = XChaChaSessionCipher::new(
        [26; 32],
        *session_id.as_uuid().as_bytes(),
        SessionDirection::ControllerToAgent,
    );
    let events = [
        InputEvent::KeyDown {
            key: KeyCode::ControlLeft,
        },
        InputEvent::KeyDown { key: KeyCode::KeyC },
    ];

    for (sequence, event) in (0_u64..).zip(events) {
        let envelope = MessageEnvelope::new(session_id, sequence, 1_234, Message::Input(event));
        let encoded = encode_wire(&envelope)?;
        let encrypted = cipher.seal(sequence, &encoded)?;
        let mut wire_payload = sequence.to_be_bytes().to_vec();
        wire_payload.extend_from_slice(&encrypted);
        controller
            .send(&RelayClientMessage::Payload(wire_payload))
            .await?;
        let RelayServerMessage::Payload(relayed) = agent.receive_significant().await? else {
            return Err("expected relayed keyboard payload".into());
        };
        let received_sequence = u64::from_be_bytes(relayed[..8].try_into()?);
        assert_eq!(received_sequence, sequence);
        let plaintext = cipher.open(received_sequence, &relayed[8..])?;
        let received: MessageEnvelope = decode_wire(&plaintext)?;
        assert_eq!(received, envelope);
    }
    harness.stop().await
}

#[tokio::test]
async fn encrypted_clipboard_crosses_relay_bidirectionally() -> TestResult {
    let harness = Harness::start(Harness::limits())?;
    let session_id = SessionId::new();
    let (mut controller, mut agent) = harness
        .pair(
            session_id,
            SessionToken::from_bytes([27; 32]),
            SessionToken::from_bytes([28; 32]),
        )
        .await?;
    let key = [29; 32];
    relay_encrypted_clipboard(
        &mut controller,
        &mut agent,
        session_id,
        key,
        SessionDirection::ControllerToAgent,
        ClipboardOrigin::Controller,
        "控制器剪贴板",
    )
    .await?;
    relay_encrypted_clipboard(
        &mut agent,
        &mut controller,
        session_id,
        key,
        SessionDirection::AgentToController,
        ClipboardOrigin::Agent,
        "エージェント",
    )
    .await?;
    harness.stop().await
}

async fn relay_encrypted_clipboard(
    sender: &mut TestPeer,
    receiver: &mut TestPeer,
    session_id: SessionId,
    key: [u8; 32],
    direction: SessionDirection,
    origin: ClipboardOrigin,
    text: &str,
) -> TestResult {
    let envelope = MessageEnvelope::new(
        session_id,
        0,
        1_234,
        Message::Clipboard(ClipboardMessage::Text {
            origin,
            revision: 1,
            text: text.to_owned(),
        }),
    );
    let plaintext = encode_wire(&envelope)?;
    let cipher = XChaChaSessionCipher::new(key, *session_id.as_uuid().as_bytes(), direction);
    let ciphertext = cipher.seal(0, &plaintext)?;
    let mut payload = 0_u64.to_be_bytes().to_vec();
    payload.extend_from_slice(&ciphertext);
    sender.send(&RelayClientMessage::Payload(payload)).await?;
    let RelayServerMessage::Payload(relayed) = receiver.receive_significant().await? else {
        return Err("expected relayed clipboard payload".into());
    };
    assert_ne!(&relayed[8..], plaintext);
    let decoded: MessageEnvelope = decode_wire(&cipher.open(0, &relayed[8..])?)?;
    assert_eq!(decoded, envelope);
    Ok(())
}

#[tokio::test]
async fn encrypted_file_transfer_crosses_relay_bidirectionally() -> TestResult {
    let limits = RelayLimits {
        maximum_frame_size: 128 * 1024,
        ..Harness::limits()
    };
    let harness = Harness::start(limits)?;
    let session_id = SessionId::new();
    let (mut controller, mut agent) = harness
        .pair(
            session_id,
            SessionToken::from_bytes([30; 32]),
            SessionToken::from_bytes([31; 32]),
        )
        .await?;
    let transfer_id = TransferId::new();
    let key = [32; 32];
    let controller_cipher = XChaChaSessionCipher::new(
        key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::ControllerToAgent,
    );
    let agent_cipher = XChaChaSessionCipher::new(
        key,
        *session_id.as_uuid().as_bytes(),
        SessionDirection::AgentToController,
    );

    relay_encrypted_file_message(
        &mut controller,
        &mut agent,
        session_id,
        &controller_cipher,
        0,
        FileTransferMessage::Start {
            transfer_id,
            direction: FileTransferDirection::Upload,
            filename: "hello.bin".into(),
            source_path: String::new(),
            destination_path: "/Data/hello.bin".into(),
            total_size: 5,
            chunk_size: 64 * 1024,
            sha256: [5; 32],
        },
    )
    .await?;
    relay_encrypted_file_message(
        &mut agent,
        &mut controller,
        session_id,
        &agent_cipher,
        0,
        FileTransferMessage::Accept {
            transfer_id,
            next_offset: 0,
        },
    )
    .await?;
    harness.stop().await
}

async fn relay_encrypted_file_message(
    sender: &mut TestPeer,
    receiver: &mut TestPeer,
    session_id: SessionId,
    cipher: &XChaChaSessionCipher,
    sequence: u64,
    message: FileTransferMessage,
) -> TestResult {
    let envelope =
        MessageEnvelope::new(session_id, sequence, 1_234, Message::FileTransfer(message));
    let plaintext = encode_wire(&envelope)?;
    let ciphertext = cipher.seal(sequence, &plaintext)?;
    let mut payload = sequence.to_be_bytes().to_vec();
    payload.extend_from_slice(&ciphertext);
    sender.send(&RelayClientMessage::Payload(payload)).await?;
    let RelayServerMessage::Payload(relayed) = receiver.receive_significant().await? else {
        return Err("expected relayed file payload".into());
    };
    assert_ne!(&relayed[8..], plaintext);
    let decoded: MessageEnvelope = decode_wire(&cipher.open(sequence, &relayed[8..])?)?;
    assert_eq!(decoded, envelope);
    Ok(())
}
