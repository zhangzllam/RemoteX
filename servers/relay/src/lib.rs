//! Authenticated QUIC session pairing and opaque frame forwarding.

use quinn::{Endpoint, RecvStream, SendStream};
use remotex_protocol::{PROTOCOL_VERSION, RelayHandshake, Role, SessionId, SessionToken};
use remotex_transport::{TransportError, read_frame, write_frame};
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{sync::Mutex, time::timeout};
use tracing::{info, warn};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CredentialKey {
    session_id: SessionId,
    role: Role,
}

#[derive(Clone, Debug)]
struct Grant {
    token: SessionToken,
    expires_at_ms: u64,
}

/// In-memory one-time credential store used by the M1 relay boundary.
#[derive(Clone, Default)]
pub struct SessionAuthorizer {
    grants: Arc<Mutex<HashMap<CredentialKey, Grant>>>,
}

impl SessionAuthorizer {
    pub async fn grant(
        &self,
        session_id: SessionId,
        role: Role,
        token: SessionToken,
        expires_at_ms: u64,
    ) {
        self.grants.lock().await.insert(
            CredentialKey { session_id, role },
            Grant {
                token,
                expires_at_ms,
            },
        );
    }

    async fn consume(&self, handshake: &RelayHandshake, now_ms: u64) -> Result<(), RelayError> {
        if handshake.version != PROTOCOL_VERSION {
            return Err(RelayError::UnsupportedVersion(handshake.version));
        }

        let key = CredentialKey {
            session_id: handshake.session_id,
            role: handshake.role,
        };
        let mut grants = self.grants.lock().await;
        let grant = grants.get(&key).ok_or(RelayError::Unauthorized)?;
        if now_ms > grant.expires_at_ms
            || !bool::from(grant.token.as_bytes().ct_eq(handshake.token.as_bytes()))
        {
            return Err(RelayError::Unauthorized);
        }
        grants.remove(&key);
        Ok(())
    }
}

/// Runtime limits applied before and during a relay session.
#[derive(Clone, Copy, Debug)]
pub struct RelayLimits {
    pub maximum_frame_size: usize,
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            maximum_frame_size: remotex_transport::DEFAULT_MAX_FRAME_SIZE,
            handshake_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(30),
        }
    }
}

struct Peer {
    role: Role,
    send: SendStream,
    receive: RecvStream,
}

struct PeerPair {
    controller: Peer,
    agent: Peer,
}

#[derive(Default)]
struct PairingRegistry {
    waiting: Mutex<HashMap<SessionId, Peer>>,
}

impl PairingRegistry {
    async fn join(
        &self,
        session_id: SessionId,
        peer: Peer,
    ) -> Result<Option<PeerPair>, RelayError> {
        let mut waiting = self.waiting.lock().await;
        if let Some(existing) = waiting.remove(&session_id) {
            if existing.role == peer.role {
                waiting.insert(session_id, existing);
                return Err(RelayError::DuplicateRole);
            }
            let pair = match peer.role {
                Role::Controller => PeerPair {
                    controller: peer,
                    agent: existing,
                },
                Role::Agent => PeerPair {
                    controller: existing,
                    agent: peer,
                },
            };
            Ok(Some(pair))
        } else {
            waiting.insert(session_id, peer);
            Ok(None)
        }
    }
}

/// A lightweight relay that authenticates a single stream per QUIC peer.
#[derive(Clone)]
pub struct RelayServer {
    authorizer: SessionAuthorizer,
    pairing: Arc<PairingRegistry>,
    limits: RelayLimits,
}

impl RelayServer {
    #[must_use]
    pub fn new(authorizer: SessionAuthorizer, limits: RelayLimits) -> Self {
        Self {
            authorizer,
            pairing: Arc::new(PairingRegistry::default()),
            limits,
        }
    }

    /// Accepts connections until `shutdown` resolves.
    pub async fn serve_until<F>(&self, endpoint: Endpoint, shutdown: F) -> Result<(), RelayError>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => {
                    endpoint.close(0_u32.into(), b"relay shutdown");
                    return Ok(());
                }
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { return Ok(()); };
                    let relay = self.clone();
                    tokio::spawn(async move {
                        if let Err(error) = relay.handle_incoming(incoming).await {
                            warn!(%error, "relay peer disconnected");
                        }
                    });
                }
            }
        }
    }

    async fn handle_incoming(&self, incoming: quinn::Incoming) -> Result<(), RelayError> {
        let connection = incoming.await?;
        let (send, mut receive) = timeout(self.limits.handshake_timeout, connection.accept_bi())
            .await
            .map_err(|_| RelayError::HandshakeTimeout)??;
        let encoded = timeout(
            self.limits.handshake_timeout,
            read_frame(&mut receive, 1024),
        )
        .await
        .map_err(|_| RelayError::HandshakeTimeout)??;
        let (handshake, consumed): (RelayHandshake, usize) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
                .map_err(|error| RelayError::InvalidHandshake(error.to_string()))?;
        if consumed != encoded.len() {
            return Err(RelayError::InvalidHandshake(
                "trailing handshake bytes".to_owned(),
            ));
        }
        self.authorizer
            .consume(&handshake, unix_timestamp_ms()?)
            .await?;
        let session_id = handshake.session_id;
        let role = handshake.role;
        info!(%session_id, ?role, "relay peer authenticated");

        if let Some(pair) = self
            .pairing
            .join(
                session_id,
                Peer {
                    role,
                    send,
                    receive,
                },
            )
            .await?
        {
            info!(%session_id, "relay session paired");
            let limits = self.limits;
            tokio::spawn(async move {
                if let Err(error) = forward_pair(pair, limits).await {
                    warn!(%session_id, %error, "relay session ended");
                }
            });
        }
        Ok(())
    }
}

async fn forward_pair(pair: PeerPair, limits: RelayLimits) -> Result<(), RelayError> {
    let controller_to_agent = forward_direction(pair.controller.receive, pair.agent.send, limits);
    let agent_to_controller = forward_direction(pair.agent.receive, pair.controller.send, limits);
    tokio::try_join!(controller_to_agent, agent_to_controller)?;
    Ok(())
}

async fn forward_direction(
    mut receive: RecvStream,
    mut send: SendStream,
    limits: RelayLimits,
) -> Result<(), RelayError> {
    loop {
        let frame = timeout(
            limits.idle_timeout,
            read_frame(&mut receive, limits.maximum_frame_size),
        )
        .await
        .map_err(|_| RelayError::IdleTimeout)??;
        write_frame(&mut send, &frame, limits.maximum_frame_size).await?;
    }
}

fn unix_timestamp_ms() -> Result<u64, RelayError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| RelayError::Clock)?;
    u64::try_from(duration.as_millis()).map_err(|_| RelayError::Clock)
}

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("relay credential is missing, expired, already used, or invalid")]
    Unauthorized,
    #[error("the same role attempted to join a session twice")]
    DuplicateRole,
    #[error("handshake timed out")]
    HandshakeTimeout,
    #[error("session was idle for too long")]
    IdleTimeout,
    #[error("invalid handshake: {0}")]
    InvalidHandshake(String),
    #[error("system clock is before the Unix epoch or out of range")]
    Clock,
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Connection(#[from] quinn::ConnectionError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn credentials_are_role_bound_and_one_time() {
        let authorizer = SessionAuthorizer::default();
        let session_id = SessionId::new();
        let token = SessionToken::from_bytes([7; 32]);
        authorizer
            .grant(session_id, Role::Agent, token.clone(), 200)
            .await;
        let wrong_role = RelayHandshake::new(session_id, Role::Controller, token.clone());
        assert!(matches!(
            authorizer.consume(&wrong_role, 100).await,
            Err(RelayError::Unauthorized)
        ));
        let valid = RelayHandshake::new(session_id, Role::Agent, token);
        authorizer.consume(&valid, 100).await.expect("valid grant");
        assert!(matches!(
            authorizer.consume(&valid, 100).await,
            Err(RelayError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn expired_credentials_are_rejected() {
        let authorizer = SessionAuthorizer::default();
        let session_id = SessionId::new();
        let token = SessionToken::from_bytes([9; 32]);
        authorizer
            .grant(session_id, Role::Controller, token.clone(), 99)
            .await;
        let handshake = RelayHandshake::new(session_id, Role::Controller, token);
        assert!(matches!(
            authorizer.consume(&handshake, 100).await,
            Err(RelayError::Unauthorized)
        ));
    }
}
