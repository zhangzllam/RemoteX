//! Transport-neutral asynchronous byte-frame contracts.

use async_trait::async_trait;
use bytes::Bytes;
use hmac::{Hmac, Mac};
use remotex_protocol::{
    ConnectivityCandidate, ConnectivityCandidateKind, DirectClientHello, DirectServerHello,
    PROTOCOL_VERSION, RelayClientMessage, RelayServerMessage, SessionCloseReason, SessionId,
    decode_wire, encode_wire,
};
use sha2::Sha256;
use std::{net::SocketAddr, time::Duration};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Default maximum application frame accepted by transport readers (8 MiB).
pub const DEFAULT_MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub server_name: String,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("connection closed")]
    Closed,
    #[error("frame size {actual} exceeds limit {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("transport failed: {0}")]
    Other(String),
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("QUIC read failed: {0}")]
    QuicRead(#[from] quinn::ReadError),
    #[error("QUIC write failed: {0}")]
    QuicWrite(#[from] quinn::WriteError),
    #[error("QUIC stream is already closed: {0}")]
    QuicClosed(#[from] quinn::ClosedStream),
    #[error("direct peer authentication failed")]
    DirectAuthentication,
    #[error("direct connectivity candidate is invalid")]
    InvalidCandidate,
    #[error("direct protocol failed: {0}")]
    DirectProtocol(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectivityState {
    SessionCreated,
    ConnectingRelay,
    RelayConnected,
    TryingDirect,
    DirectConnected,
    DirectFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCandidate {
    pub kind: ConnectivityCandidateKind,
    pub address: SocketAddr,
    pub server_name: String,
    pub priority: u16,
}

pub fn resolve_candidates(
    candidates: &[ConnectivityCandidate],
) -> Result<Vec<ResolvedCandidate>, TransportError> {
    if candidates.len() > remotex_protocol::MAX_CONNECTIVITY_CANDIDATES {
        return Err(TransportError::InvalidCandidate);
    }
    let mut resolved = candidates
        .iter()
        .map(|candidate| {
            candidate
                .validate()
                .map_err(|_| TransportError::InvalidCandidate)?;
            Ok::<ResolvedCandidate, TransportError>(ResolvedCandidate {
                kind: candidate.kind,
                address: candidate
                    .address
                    .parse()
                    .map_err(|_| TransportError::InvalidCandidate)?,
                server_name: candidate.server_name.clone(),
                priority: candidate.priority,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    resolved.sort_by_key(|candidate| {
        (
            match candidate.kind {
                ConnectivityCandidateKind::Lan => 0_u8,
                ConnectivityCandidateKind::ServerReflexive => 1,
            },
            std::cmp::Reverse(candidate.priority),
        )
    });
    Ok(resolved)
}

/// Reads one big-endian length-prefixed frame with a pre-allocation size check.
pub async fn read_frame<R>(reader: &mut R, maximum: usize) -> Result<Bytes, TransportError>
where
    R: AsyncRead + Unpin,
{
    let length = reader.read_u32().await? as usize;
    if length > maximum {
        return Err(TransportError::FrameTooLarge {
            actual: length,
            maximum,
        });
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(Bytes::from(payload))
}

/// Writes one big-endian length-prefixed frame after enforcing the size limit.
pub async fn write_frame<W>(
    writer: &mut W,
    frame: &[u8],
    maximum: usize,
) -> Result<(), TransportError>
where
    W: AsyncWrite + Unpin,
{
    if frame.len() > maximum {
        return Err(TransportError::FrameTooLarge {
            actual: frame.len(),
            maximum,
        });
    }
    let length = u32::try_from(frame.len()).map_err(|_| TransportError::FrameTooLarge {
        actual: frame.len(),
        maximum,
    })?;
    writer.write_u32(length).await?;
    writer.write_all(frame).await?;
    writer.flush().await?;
    Ok(())
}

/// A single framed bidirectional stream carried by a QUIC connection.
pub struct QuicFrameConnection {
    send: quinn::SendStream,
    receive: quinn::RecvStream,
    maximum: usize,
}

impl QuicFrameConnection {
    #[must_use]
    pub const fn new(send: quinn::SendStream, receive: quinn::RecvStream, maximum: usize) -> Self {
        Self {
            send,
            receive,
            maximum,
        }
    }
}

#[async_trait]
impl Connection for QuicFrameConnection {
    async fn send(&mut self, frame: Bytes) -> Result<(), TransportError> {
        write_frame(&mut self.send, &frame, self.maximum).await
    }

    async fn receive(&mut self) -> Result<Bytes, TransportError> {
        read_frame(&mut self.receive, self.maximum).await
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.send.finish()?;
        Ok(())
    }
}

/// Adapts a mutually authenticated peer-to-peer stream to the Relay-shaped
/// application framing used by existing Agent and Controller loops.
pub struct DirectPeerConnection {
    inner: QuicFrameConnection,
}

impl DirectPeerConnection {
    #[must_use]
    pub const fn new(inner: QuicFrameConnection) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Connection for DirectPeerConnection {
    async fn send(&mut self, frame: Bytes) -> Result<(), TransportError> {
        let outbound = match decode_wire::<RelayClientMessage>(&frame)
            .map_err(|error| TransportError::DirectProtocol(error.to_string()))?
        {
            RelayClientMessage::Payload(payload) => RelayServerMessage::Payload(payload),
            RelayClientMessage::Heartbeat { nonce }
            | RelayClientMessage::HeartbeatAck { nonce } => {
                RelayServerMessage::HeartbeatAck { nonce }
            }
            RelayClientMessage::Close => RelayServerMessage::SessionClosed {
                reason: SessionCloseReason::ClientClosed,
            },
            RelayClientMessage::ClientHello(_) => {
                return Err(TransportError::DirectProtocol(
                    "Relay ClientHello is invalid after direct authentication".to_owned(),
                ));
            }
        };
        let encoded = encode_wire(&outbound)
            .map_err(|error| TransportError::DirectProtocol(error.to_string()))?;
        self.inner.send(Bytes::from(encoded)).await
    }

    async fn receive(&mut self) -> Result<Bytes, TransportError> {
        self.inner.receive().await
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.inner.close().await
    }
}

pub async fn authenticate_direct_client(
    connection: &mut QuicFrameConnection,
    session_id: SessionId,
    session_key: &[u8; 32],
) -> Result<(), TransportError> {
    let client_nonce: [u8; 32] = rand::random();
    let hello = DirectClientHello {
        version: PROTOCOL_VERSION,
        session_id,
        client_nonce,
        proof: direct_proof(session_key, b"controller", session_id, &client_nonce, &[]),
    };
    let encoded =
        encode_wire(&hello).map_err(|error| TransportError::DirectProtocol(error.to_string()))?;
    connection.send(Bytes::from(encoded)).await?;
    let response: DirectServerHello = decode_wire(&connection.receive().await?)
        .map_err(|error| TransportError::DirectProtocol(error.to_string()))?;
    if response.version != PROTOCOL_VERSION || response.session_id != session_id {
        return Err(TransportError::DirectAuthentication);
    }
    verify_direct_proof(
        session_key,
        b"agent",
        session_id,
        &client_nonce,
        &response.server_nonce,
        &response.proof,
    )
}

pub async fn authenticate_direct_server(
    connection: &mut QuicFrameConnection,
    session_id: SessionId,
    session_key: &[u8; 32],
) -> Result<(), TransportError> {
    let hello: DirectClientHello = decode_wire(&connection.receive().await?)
        .map_err(|error| TransportError::DirectProtocol(error.to_string()))?;
    if hello.version != PROTOCOL_VERSION || hello.session_id != session_id {
        return Err(TransportError::DirectAuthentication);
    }
    verify_direct_proof(
        session_key,
        b"controller",
        session_id,
        &hello.client_nonce,
        &[],
        &hello.proof,
    )?;
    let server_nonce: [u8; 32] = rand::random();
    let response = DirectServerHello {
        version: PROTOCOL_VERSION,
        session_id,
        server_nonce,
        proof: direct_proof(
            session_key,
            b"agent",
            session_id,
            &hello.client_nonce,
            &server_nonce,
        ),
    };
    let encoded = encode_wire(&response)
        .map_err(|error| TransportError::DirectProtocol(error.to_string()))?;
    connection.send(Bytes::from(encoded)).await
}

pub async fn connect_direct_candidates(
    endpoint: &quinn::Endpoint,
    candidates: &[ConnectivityCandidate],
    session_id: SessionId,
    session_key: &[u8; 32],
    attempt_timeout: Duration,
) -> Result<(DirectPeerConnection, ConnectivityCandidateKind), TransportError> {
    let candidates = resolve_candidates(candidates)?;
    if candidates.is_empty() {
        return Err(TransportError::InvalidCandidate);
    }
    tokio::time::timeout(attempt_timeout, async {
        loop {
            for candidate in &candidates {
                if let Ok(connection) =
                    connect_direct_candidate(endpoint, candidate, session_id, session_key).await
                {
                    return Ok((connection, candidate.kind));
                }
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    })
    .await
    .map_err(|_| TransportError::Closed)?
}

async fn connect_direct_candidate(
    endpoint: &quinn::Endpoint,
    candidate: &ResolvedCandidate,
    session_id: SessionId,
    session_key: &[u8; 32],
) -> Result<DirectPeerConnection, TransportError> {
    let connection = endpoint
        .connect(candidate.address, &candidate.server_name)
        .map_err(|error| TransportError::Other(error.to_string()))?
        .await
        .map_err(|error| TransportError::Other(error.to_string()))?;
    let (send, receive) = connection
        .open_bi()
        .await
        .map_err(|error| TransportError::Other(error.to_string()))?;
    let mut framed = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
    authenticate_direct_client(&mut framed, session_id, session_key).await?;
    Ok(DirectPeerConnection::new(framed))
}

fn direct_proof(
    session_key: &[u8; 32],
    role: &[u8],
    session_id: SessionId,
    client_nonce: &[u8; 32],
    server_nonce: &[u8],
) -> [u8; 32] {
    let mac = direct_mac(session_key, role, session_id, client_nonce, server_nonce);
    mac.finalize().into_bytes().into()
}

fn verify_direct_proof(
    session_key: &[u8; 32],
    role: &[u8],
    session_id: SessionId,
    client_nonce: &[u8; 32],
    server_nonce: &[u8],
    proof: &[u8; 32],
) -> Result<(), TransportError> {
    direct_mac(session_key, role, session_id, client_nonce, server_nonce)
        .verify_slice(proof)
        .map_err(|_| TransportError::DirectAuthentication)
}

fn direct_mac(
    session_key: &[u8; 32],
    role: &[u8],
    session_id: SessionId,
    client_nonce: &[u8; 32],
    server_nonce: &[u8],
) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(session_key)
        .expect("HMAC accepts a fixed 256-bit Session key");
    mac.update(b"RemoteX/direct-auth/v1");
    mac.update(role);
    mac.update(session_id.as_uuid().as_bytes());
    mac.update(client_nonce);
    mac.update(server_nonce);
    mac
}

pub const DEFAULT_DIRECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);

#[async_trait]
pub trait Connection: Send {
    async fn send(&mut self, frame: Bytes) -> Result<(), TransportError>;
    async fn receive(&mut self) -> Result<Bytes, TransportError>;
    async fn close(&mut self) -> Result<(), TransportError>;
}

#[async_trait]
pub trait Connector: Send + Sync {
    type Connection: Connection;
    async fn connect(&self, endpoint: &Endpoint) -> Result<Self::Connection, TransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn::{ClientConfig, Endpoint as QuinnEndpoint, ServerConfig};
    use rcgen::generate_simple_self_signed;
    use rustls::RootCertStore;
    use std::{collections::VecDeque, sync::Arc, time::Instant};

    struct MemoryConnection {
        frames: VecDeque<Bytes>,
        closed: bool,
    }

    #[async_trait]
    impl Connection for MemoryConnection {
        async fn send(&mut self, frame: Bytes) -> Result<(), TransportError> {
            if self.closed {
                return Err(TransportError::Closed);
            }
            self.frames.push_back(frame);
            Ok(())
        }
        async fn receive(&mut self) -> Result<Bytes, TransportError> {
            self.frames.pop_front().ok_or(TransportError::Closed)
        }
        async fn close(&mut self) -> Result<(), TransportError> {
            self.closed = true;
            Ok(())
        }
    }

    #[tokio::test]
    async fn contract_can_be_implemented_without_networking() {
        let mut connection = MemoryConnection {
            frames: VecDeque::new(),
            closed: false,
        };
        connection
            .send(Bytes::from_static(b"frame"))
            .await
            .expect("send");
        assert_eq!(
            connection.receive().await.expect("receive"),
            Bytes::from_static(b"frame")
        );
        connection.close().await.expect("close");
        assert!(matches!(
            connection.send(Bytes::new()).await,
            Err(TransportError::Closed)
        ));
    }

    #[tokio::test]
    async fn framing_round_trips_and_rejects_oversized_frames() {
        let (mut left, mut right) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move { write_frame(&mut left, b"hello", 16).await });
        assert_eq!(
            read_frame(&mut right, 16).await.expect("read"),
            Bytes::from_static(b"hello")
        );
        writer.await.expect("join").expect("write");

        let (mut left, mut right) = tokio::io::duplex(64);
        left.write_u32(17).await.expect("length");
        assert!(matches!(
            read_frame(&mut right, 16).await,
            Err(TransportError::FrameTooLarge {
                actual: 17,
                maximum: 16
            })
        ));
    }

    #[test]
    fn candidates_prefer_lan_and_reject_invalid_addresses() {
        let candidates = vec![
            ConnectivityCandidate {
                kind: ConnectivityCandidateKind::ServerReflexive,
                address: "203.0.113.7:7444".to_owned(),
                server_name: "direct.example.test".to_owned(),
                priority: 200,
            },
            ConnectivityCandidate {
                kind: ConnectivityCandidateKind::Lan,
                address: "192.168.1.20:7444".to_owned(),
                server_name: "direct.example.test".to_owned(),
                priority: 100,
            },
        ];
        let resolved = resolve_candidates(&candidates).expect("resolve");
        assert_eq!(resolved[0].kind, ConnectivityCandidateKind::Lan);

        let mut invalid = candidates;
        invalid[0].address = "not-an-address".to_owned();
        assert!(matches!(
            resolve_candidates(&invalid),
            Err(TransportError::InvalidCandidate)
        ));
    }

    #[test]
    fn direct_proofs_bind_role_session_and_both_nonces() {
        let key = [7_u8; 32];
        let session_id = SessionId::new();
        let client_nonce = [1_u8; 32];
        let server_nonce = [2_u8; 32];
        let proof = direct_proof(&key, b"agent", session_id, &client_nonce, &server_nonce);
        assert!(
            verify_direct_proof(
                &key,
                b"agent",
                session_id,
                &client_nonce,
                &server_nonce,
                &proof,
            )
            .is_ok()
        );
        assert!(matches!(
            verify_direct_proof(
                &key,
                b"controller",
                session_id,
                &client_nonce,
                &server_nonce,
                &proof,
            ),
            Err(TransportError::DirectAuthentication)
        ));
    }

    #[tokio::test]
    async fn authenticated_lan_candidate_connects_and_reconnects() {
        for _ in 0..2 {
            let (server, client, candidate) = direct_test_endpoints();
            let session_id = SessionId::new();
            let key = [9_u8; 32];
            let server_task = tokio::spawn(async move {
                let incoming = server.accept().await.expect("accept direct");
                let connection = incoming.await.expect("direct QUIC handshake");
                let (send, receive) = connection.accept_bi().await.expect("accept stream");
                let mut framed = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
                authenticate_direct_server(&mut framed, session_id, &key)
                    .await
                    .expect("authenticate Controller");
                tokio::time::sleep(Duration::from_millis(100)).await;
            });
            let (_, kind) = connect_direct_candidates(
                &client,
                &[candidate],
                session_id,
                &key,
                Duration::from_secs(1),
            )
            .await
            .expect("connect authenticated LAN candidate");
            assert_eq!(kind, ConnectivityCandidateKind::Lan);
            server_task.await.expect("join direct server");
        }
    }

    #[tokio::test]
    async fn wrong_session_key_fails_authentication_and_timeout_is_bounded() {
        let (server, client, candidate) = direct_test_endpoints();
        let session_id = SessionId::new();
        let server_task = tokio::spawn(async move {
            let incoming = server.accept().await.expect("accept direct");
            let connection = incoming.await.expect("direct QUIC handshake");
            let (send, receive) = connection.accept_bi().await.expect("accept stream");
            let mut framed = QuicFrameConnection::new(send, receive, DEFAULT_MAX_FRAME_SIZE);
            assert!(matches!(
                authenticate_direct_server(&mut framed, session_id, &[4_u8; 32]).await,
                Err(TransportError::DirectAuthentication)
            ));
        });
        let started = Instant::now();
        assert!(
            connect_direct_candidates(
                &client,
                &[candidate],
                session_id,
                &[5_u8; 32],
                Duration::from_millis(250),
            )
            .await
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        server_task.await.expect("join direct server");
    }

    fn direct_test_endpoints() -> (QuinnEndpoint, QuinnEndpoint, ConnectivityCandidate) {
        let generated =
            generate_simple_self_signed(vec!["localhost".to_owned()]).expect("certificate");
        let certificate = generated.cert.der().clone();
        let key =
            rustls::pki_types::PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der());
        let server_config = ServerConfig::with_single_cert(vec![certificate.clone()], key.into())
            .expect("server config");
        let server =
            QuinnEndpoint::server(server_config, "127.0.0.1:0".parse().expect("server bind"))
                .expect("server endpoint");
        let mut roots = RootCertStore::empty();
        roots.add(certificate).expect("test root");
        let client_config =
            ClientConfig::with_root_certificates(Arc::new(roots)).expect("client config");
        let mut client = QuinnEndpoint::client("127.0.0.1:0".parse().expect("client bind"))
            .expect("client endpoint");
        client.set_default_client_config(client_config);
        let candidate = ConnectivityCandidate {
            kind: ConnectivityCandidateKind::Lan,
            address: server.local_addr().expect("server address").to_string(),
            server_name: "localhost".to_owned(),
            priority: 100,
        };
        (server, client, candidate)
    }
}
