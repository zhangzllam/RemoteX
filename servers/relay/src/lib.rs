//! Authenticated QUIC session pairing, liveness, and opaque payload forwarding.

use async_trait::async_trait;
use quinn::{Endpoint, RecvStream, SendStream};
use remotex_protocol::{
    ClientHello, MAX_RELAY_HANDSHAKE_SIZE, PROTOCOL_VERSION, RelayClientMessage,
    RelayProtocolErrorCode, RelayServerMessage, Role, SessionCloseReason, SessionId, SessionToken,
    decode_wire, encode_wire,
};
use remotex_transport::{TransportError, read_frame, write_frame};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{
    sync::{Mutex, Semaphore, mpsc, watch},
    task::JoinSet,
    time::{Instant, interval_at, sleep_until, timeout},
};
use tracing::{info, warn};

#[derive(Clone, Debug)]
struct RoleGrant {
    token: SessionToken,
    expires_at_ms: u64,
    consumed: bool,
}

#[derive(Clone, Debug, Default)]
struct SessionGrant {
    controller: Option<RoleGrant>,
    agent: Option<RoleGrant>,
}

impl SessionGrant {
    fn credential_mut(&mut self, role: Role) -> Option<&mut RoleGrant> {
        match role {
            Role::Controller => self.controller.as_mut(),
            Role::Agent => self.agent.as_mut(),
        }
    }

    fn opposite_credential(&self, role: Role) -> Option<&RoleGrant> {
        match role {
            Role::Controller => self.agent.as_ref(),
            Role::Agent => self.controller.as_ref(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticatedSession {
    pub session_id: SessionId,
    pub role: Role,
}

/// Replaceable authentication boundary. M7 can supply a control-server adapter.
#[async_trait]
pub trait SessionAuthenticator: Send + Sync {
    async fn authenticate(
        &self,
        hello: &ClientHello,
        now_ms: u64,
    ) -> Result<AuthenticatedSession, AuthenticationError>;
}

/// In-memory, one-time, role-bound session credentials used by M1.
#[derive(Clone, Default)]
pub struct InMemorySessionAuthenticator {
    sessions: Arc<Mutex<HashMap<SessionId, SessionGrant>>>,
}

impl InMemorySessionAuthenticator {
    pub async fn grant(
        &self,
        session_id: SessionId,
        role: Role,
        token: SessionToken,
        expires_at_ms: u64,
    ) {
        let mut sessions = self.sessions.lock().await;
        let session = sessions.entry(session_id).or_default();
        let grant = Some(RoleGrant {
            token,
            expires_at_ms,
            consumed: false,
        });
        match role {
            Role::Controller => session.controller = grant,
            Role::Agent => session.agent = grant,
        }
    }
}

#[async_trait]
impl SessionAuthenticator for InMemorySessionAuthenticator {
    async fn authenticate(
        &self,
        hello: &ClientHello,
        now_ms: u64,
    ) -> Result<AuthenticatedSession, AuthenticationError> {
        if hello.version != PROTOCOL_VERSION {
            return Err(AuthenticationError::UnsupportedVersion(hello.version));
        }

        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(&hello.session_id)
            .ok_or(AuthenticationError::UnknownSession)?;
        if session
            .opposite_credential(hello.role)
            .is_some_and(|grant| bool::from(grant.token.as_bytes().ct_eq(hello.token.as_bytes())))
        {
            return Err(AuthenticationError::RoleMismatch);
        }
        let credential = session
            .credential_mut(hello.role)
            .ok_or(AuthenticationError::InvalidToken)?;
        if now_ms > credential.expires_at_ms {
            return Err(AuthenticationError::ExpiredSession);
        }
        if !bool::from(credential.token.as_bytes().ct_eq(hello.token.as_bytes())) {
            return Err(AuthenticationError::InvalidToken);
        }
        if credential.consumed {
            return Err(AuthenticationError::TokenAlreadyUsed);
        }
        credential.consumed = true;
        Ok(AuthenticatedSession {
            session_id: hello.session_id,
            role: hello.role,
        })
    }
}

/// Compatibility name used by the existing composition root.
pub type SessionAuthorizer = InMemorySessionAuthenticator;

#[derive(Clone, Copy, Debug)]
pub struct RelayLimits {
    pub maximum_frame_size: usize,
    pub max_connections: usize,
    pub max_pending_sessions: usize,
    pub outbound_queue_capacity: usize,
    pub handshake_timeout: Duration,
    pub heartbeat_interval: Duration,
    pub peer_timeout: Duration,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            maximum_frame_size: remotex_transport::DEFAULT_MAX_FRAME_SIZE,
            max_connections: 1_024,
            max_pending_sessions: 512,
            outbound_queue_capacity: 32,
            handshake_timeout: Duration::from_secs(10),
            heartbeat_interval: Duration::from_secs(5),
            peer_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone)]
struct PeerHandle {
    connection_id: u64,
    outbound: mpsc::Sender<RelayServerMessage>,
    shutdown: watch::Sender<Option<SessionCloseReason>>,
}

#[derive(Default)]
struct SessionEntry {
    controller: Option<PeerHandle>,
    agent: Option<PeerHandle>,
}

impl SessionEntry {
    fn peer(&self, role: Role) -> Option<&PeerHandle> {
        match role {
            Role::Controller => self.controller.as_ref(),
            Role::Agent => self.agent.as_ref(),
        }
    }

    fn peer_mut(&mut self, role: Role) -> &mut Option<PeerHandle> {
        match role {
            Role::Controller => &mut self.controller,
            Role::Agent => &mut self.agent,
        }
    }

    fn opposite(&self, role: Role) -> Option<&PeerHandle> {
        match role {
            Role::Controller => self.agent.as_ref(),
            Role::Agent => self.controller.as_ref(),
        }
    }

    const fn ready(&self) -> bool {
        self.controller.is_some() && self.agent.is_some()
    }
}

#[derive(Default)]
struct SessionRegistry {
    sessions: Mutex<HashMap<SessionId, SessionEntry>>,
}

impl SessionRegistry {
    async fn register(
        &self,
        session_id: SessionId,
        role: Role,
        peer: PeerHandle,
        maximum_sessions: usize,
    ) -> Result<(), RelayError> {
        let notifications = {
            let mut sessions = self.sessions.lock().await;
            if !sessions.contains_key(&session_id) && sessions.len() >= maximum_sessions {
                return Err(RelayError::CapacityExceeded);
            }
            let session = sessions.entry(session_id).or_default();
            if session.peer(role).is_some() {
                return Err(RelayError::DuplicateRole);
            }
            *session.peer_mut(role) = Some(peer);
            if let (Some(controller), Some(agent)) = (&session.controller, &session.agent) {
                vec![
                    (controller.outbound.clone(), RelayServerMessage::PeerReady),
                    (agent.outbound.clone(), RelayServerMessage::PeerReady),
                ]
            } else {
                let waiting_for = match role {
                    Role::Controller => Role::Agent,
                    Role::Agent => Role::Controller,
                };
                let outbound = session
                    .peer(role)
                    .ok_or(RelayError::SessionNotReady)?
                    .outbound
                    .clone();
                vec![(
                    outbound,
                    RelayServerMessage::WaitingForPeer { role: waiting_for },
                )]
            }
        };

        for (outbound, message) in notifications {
            queue_message(&outbound, message)?;
        }
        Ok(())
    }

    async fn forward(
        &self,
        session_id: SessionId,
        source_role: Role,
        payload: Vec<u8>,
    ) -> Result<(), RelayError> {
        let target = {
            let sessions = self.sessions.lock().await;
            let session = sessions
                .get(&session_id)
                .ok_or(RelayError::SessionNotReady)?;
            if !session.ready() {
                return Err(RelayError::SessionNotReady);
            }
            session
                .opposite(source_role)
                .ok_or(RelayError::SessionNotReady)?
                .outbound
                .clone()
        };
        queue_message(&target, RelayServerMessage::Payload(payload))
    }

    async fn disconnect(
        &self,
        session_id: SessionId,
        role: Role,
        connection_id: u64,
        peer_reason: SessionCloseReason,
    ) {
        let peer_to_notify = {
            let mut sessions = self.sessions.lock().await;
            let Some(session) = sessions.get(&session_id) else {
                return;
            };
            if session.peer(role).map(|peer| peer.connection_id) != Some(connection_id) {
                return;
            }
            let peer = session.opposite(role).cloned();
            sessions.remove(&session_id);
            peer
        };
        if let Some(peer) = peer_to_notify {
            let _result = peer.shutdown.send(Some(peer_reason));
        }
    }

    async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    async fn peer_count(&self) -> usize {
        self.sessions
            .lock()
            .await
            .values()
            .map(|session| {
                usize::from(session.controller.is_some()) + usize::from(session.agent.is_some())
            })
            .sum()
    }
}

#[derive(Clone)]
pub struct RelayServer {
    authenticator: Arc<dyn SessionAuthenticator>,
    registry: Arc<SessionRegistry>,
    connection_slots: Arc<Semaphore>,
    connection_sequence: Arc<AtomicU64>,
    limits: RelayLimits,
}

impl RelayServer {
    #[must_use]
    pub fn new<A>(authenticator: A, limits: RelayLimits) -> Self
    where
        A: SessionAuthenticator + 'static,
    {
        Self {
            authenticator: Arc::new(authenticator),
            registry: Arc::new(SessionRegistry::default()),
            connection_slots: Arc::new(Semaphore::new(limits.max_connections)),
            connection_sequence: Arc::new(AtomicU64::new(1)),
            limits,
        }
    }

    #[must_use]
    pub fn limits(&self) -> RelayLimits {
        self.limits
    }

    pub async fn active_session_count(&self) -> usize {
        self.registry.session_count().await
    }

    pub async fn connected_peer_count(&self) -> usize {
        self.registry.peer_count().await
    }

    /// Accepts connections until `shutdown` resolves and owns all peer tasks.
    pub async fn serve_until<F>(&self, endpoint: Endpoint, shutdown: F) -> Result<(), RelayError>
    where
        F: Future<Output = ()>,
    {
        let mut tasks = JoinSet::new();
        tokio::pin!(shutdown);
        info!(event = "relay_started", address = %endpoint.local_addr()?);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { break; };
                    let relay = self.clone();
                    tasks.spawn(async move {
                        if let Err(error) = relay.handle_incoming(incoming).await {
                            warn!(event = "peer_disconnected", %error);
                        }
                    });
                }
                joined = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(Err(error)) = joined {
                        warn!(event = "relay_task_failed", %error);
                    }
                }
            }
        }

        endpoint.close(0_u32.into(), b"relay shutdown");
        let drain = async {
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    warn!(event = "relay_task_failed", %error);
                }
            }
        };
        if timeout(self.limits.peer_timeout, drain).await.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        Ok(())
    }

    async fn handle_incoming(&self, incoming: quinn::Incoming) -> Result<(), RelayError> {
        let connection = incoming.await?;
        let connection_id = self.connection_sequence.fetch_add(1, Ordering::Relaxed);
        info!(event = "connection_received", connection_id, remote = %connection.remote_address());
        let Ok(_permit) = self.connection_slots.clone().try_acquire_owned() else {
            connection.close(1_u32.into(), b"relay connection capacity exceeded");
            return Err(RelayError::CapacityExceeded);
        };

        let (mut send, mut receive) =
            timeout(self.limits.handshake_timeout, connection.accept_bi())
                .await
                .map_err(|_| RelayError::HandshakeTimeout)??;
        let encoded = timeout(
            self.limits.handshake_timeout,
            read_frame(&mut receive, MAX_RELAY_HANDSHAKE_SIZE),
        )
        .await
        .map_err(|_| RelayError::HandshakeTimeout)??;
        let client_message: RelayClientMessage = match decode_wire(&encoded) {
            Ok(message) => message,
            Err(error) => {
                let relay_error = RelayError::InvalidClientMessage(error.to_string());
                send_rejection(&mut send, &relay_error).await;
                connection.close(2_u32.into(), b"invalid relay handshake");
                return Err(relay_error);
            }
        };
        let RelayClientMessage::ClientHello(hello) = client_message else {
            let error =
                RelayError::InvalidClientMessage("first message must be ClientHello".to_owned());
            send_rejection(&mut send, &error).await;
            connection.close(2_u32.into(), b"missing client hello");
            return Err(error);
        };
        let authenticated = match self
            .authenticator
            .authenticate(&hello, unix_timestamp_ms()?)
            .await
        {
            Ok(authenticated) => authenticated,
            Err(error) => {
                let relay_error = RelayError::Authentication(error);
                warn!(
                    event = "peer_rejected",
                    connection_id,
                    session_id = %hello.session_id,
                    role = ?hello.role,
                    reason = %relay_error
                );
                send_rejection(&mut send, &relay_error).await;
                connection.close(3_u32.into(), b"relay authentication failed");
                return Err(relay_error);
            }
        };
        info!(
            event = "peer_authenticated",
            connection_id,
            session_id = %authenticated.session_id,
            role = ?authenticated.role
        );
        self.run_peer(connection, connection_id, authenticated, send, receive)
            .await
    }

    async fn run_peer(
        &self,
        connection: quinn::Connection,
        connection_id: u64,
        authenticated: AuthenticatedSession,
        send: SendStream,
        receive: RecvStream,
    ) -> Result<(), RelayError> {
        let queue_capacity = self.limits.outbound_queue_capacity.max(1);
        let (outbound, outbound_receiver) = mpsc::channel(queue_capacity);
        let (shutdown, shutdown_receiver) = watch::channel(None);
        let mut writer = tokio::spawn(writer_loop(
            send,
            outbound_receiver,
            self.limits.maximum_frame_size,
        ));
        let registration = self
            .registry
            .register(
                authenticated.session_id,
                authenticated.role,
                PeerHandle {
                    connection_id,
                    outbound: outbound.clone(),
                    shutdown,
                },
                self.limits.max_pending_sessions,
            )
            .await;
        if let Err(error) = registration {
            warn!(
                event = "peer_rejected",
                connection_id,
                session_id = %authenticated.session_id,
                role = ?authenticated.role,
                reason = %error
            );
            send_terminal(
                &outbound,
                protocol_error_message(&error),
                self.limits.handshake_timeout,
            )
            .await;
            drop(outbound);
            finish_writer(&mut writer, self.limits.handshake_timeout).await;
            connection.close(4_u32.into(), b"relay registration rejected");
            return Err(error);
        }

        let is_ready = self
            .registry
            .sessions
            .lock()
            .await
            .get(&authenticated.session_id)
            .is_some_and(SessionEntry::ready);
        info!(
            event = if is_ready { "session_ready" } else { "session_waiting" },
            connection_id,
            session_id = %authenticated.session_id,
            role = ?authenticated.role
        );

        let outcome = self
            .peer_loop(
                authenticated,
                connection_id,
                receive,
                &outbound,
                shutdown_receiver,
            )
            .await;
        let peer_reason = peer_notification_reason(&outcome);
        self.registry
            .disconnect(
                authenticated.session_id,
                authenticated.role,
                connection_id,
                peer_reason,
            )
            .await;
        if let Some(message) = terminal_message(&outcome) {
            send_terminal(&outbound, message, self.limits.handshake_timeout).await;
        }
        drop(outbound);
        finish_writer(&mut writer, self.limits.handshake_timeout).await;
        connection.close(0_u32.into(), b"relay session closed");
        info!(
            event = "session_closed",
            connection_id,
            session_id = %authenticated.session_id,
            role = ?authenticated.role
        );
        outcome.map(|_| ())
    }

    async fn peer_loop(
        &self,
        authenticated: AuthenticatedSession,
        connection_id: u64,
        mut receive: RecvStream,
        outbound: &mpsc::Sender<RelayServerMessage>,
        mut shutdown: watch::Receiver<Option<SessionCloseReason>>,
    ) -> Result<LoopEnd, RelayError> {
        let heartbeat_interval = self.limits.heartbeat_interval.max(Duration::from_millis(1));
        let mut heartbeat = interval_at(Instant::now() + heartbeat_interval, heartbeat_interval);
        let inactivity = sleep_until(Instant::now() + self.limits.peer_timeout);
        tokio::pin!(inactivity);
        let mut heartbeat_nonce = 0_u64;

        loop {
            tokio::select! {
                frame = read_frame(&mut receive, self.limits.maximum_frame_size) => {
                    let frame = frame?;
                    let message: RelayClientMessage = decode_wire(&frame)
                        .map_err(|error| RelayError::InvalidClientMessage(error.to_string()))?;
                    inactivity.as_mut().reset(Instant::now() + self.limits.peer_timeout);
                    match message {
                        RelayClientMessage::Payload(payload) => {
                            self.registry
                                .forward(authenticated.session_id, authenticated.role, payload)
                                .await?;
                        }
                        RelayClientMessage::Heartbeat { nonce } => {
                            queue_message(outbound, RelayServerMessage::HeartbeatAck { nonce })?;
                        }
                        RelayClientMessage::HeartbeatAck { .. } => {}
                        RelayClientMessage::Close => return Ok(LoopEnd::ClientClosed),
                        RelayClientMessage::ClientHello(_) => {
                            return Err(RelayError::InvalidClientMessage(
                                "ClientHello may only be sent once".to_owned(),
                            ));
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    heartbeat_nonce = heartbeat_nonce.wrapping_add(1);
                    queue_message(
                        outbound,
                        RelayServerMessage::Heartbeat { nonce: heartbeat_nonce },
                    )?;
                }
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        return Ok(LoopEnd::PeerClosed(SessionCloseReason::PeerDisconnected));
                    }
                    if let Some(reason) = *shutdown.borrow() {
                        return Ok(LoopEnd::PeerClosed(reason));
                    }
                }
                () = &mut inactivity => {
                    warn!(
                        event = "heartbeat_timeout",
                        connection_id,
                        session_id = %authenticated.session_id,
                        role = ?authenticated.role
                    );
                    return Err(RelayError::PeerTimeout);
                }
            }
        }
    }
}

async fn writer_loop(
    mut send: SendStream,
    mut outbound: mpsc::Receiver<RelayServerMessage>,
    maximum_frame_size: usize,
) -> Result<(), RelayError> {
    while let Some(message) = outbound.recv().await {
        let encoded = encode_wire(&message)
            .map_err(|error| RelayError::InvalidServerMessage(error.to_string()))?;
        write_frame(&mut send, &encoded, maximum_frame_size).await?;
    }
    send.finish()?;
    let _result = send.stopped().await;
    Ok(())
}

async fn finish_writer(
    writer: &mut tokio::task::JoinHandle<Result<(), RelayError>>,
    wait: Duration,
) {
    if timeout(wait, &mut *writer).await.is_err() {
        writer.abort();
    }
}

async fn send_terminal(
    outbound: &mpsc::Sender<RelayServerMessage>,
    message: RelayServerMessage,
    wait: Duration,
) {
    let _result = timeout(wait, outbound.send(message)).await;
}

async fn send_rejection(send: &mut SendStream, error: &RelayError) {
    let Some(message) = error.protocol_error() else {
        return;
    };
    let Ok(encoded) = encode_wire(&message) else {
        return;
    };
    let _result = write_frame(send, &encoded, MAX_RELAY_HANDSHAKE_SIZE).await;
    let _result = send.finish();
    let _result = timeout(Duration::from_secs(1), send.stopped()).await;
}

fn queue_message(
    outbound: &mpsc::Sender<RelayServerMessage>,
    message: RelayServerMessage,
) -> Result<(), RelayError> {
    outbound.try_send(message).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => RelayError::SlowConsumer,
        mpsc::error::TrySendError::Closed(_) => RelayError::PeerDisconnected,
    })
}

fn protocol_error_message(error: &RelayError) -> RelayServerMessage {
    error
        .protocol_error()
        .unwrap_or(RelayServerMessage::ProtocolError {
            code: RelayProtocolErrorCode::MalformedMessage,
            message: "relay rejected the connection".to_owned(),
        })
}

fn peer_notification_reason(outcome: &Result<LoopEnd, RelayError>) -> SessionCloseReason {
    match outcome {
        Ok(LoopEnd::PeerClosed(reason)) => *reason,
        Err(RelayError::PeerTimeout) => SessionCloseReason::HeartbeatTimeout,
        Err(RelayError::SlowConsumer) => SessionCloseReason::SlowConsumer,
        Err(RelayError::InvalidClientMessage(_) | RelayError::SessionNotReady) => {
            SessionCloseReason::ProtocolViolation
        }
        Ok(LoopEnd::ClientClosed) | Err(_) => SessionCloseReason::PeerDisconnected,
    }
}

fn terminal_message(outcome: &Result<LoopEnd, RelayError>) -> Option<RelayServerMessage> {
    match outcome {
        Ok(LoopEnd::ClientClosed) => Some(RelayServerMessage::SessionClosed {
            reason: SessionCloseReason::ClientClosed,
        }),
        Ok(LoopEnd::PeerClosed(reason)) => {
            Some(RelayServerMessage::SessionClosed { reason: *reason })
        }
        Err(RelayError::PeerTimeout) => Some(RelayServerMessage::SessionClosed {
            reason: SessionCloseReason::HeartbeatTimeout,
        }),
        Err(error) => error.protocol_error(),
    }
}

fn unix_timestamp_ms() -> Result<u64, RelayError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| RelayError::Clock)?;
    u64::try_from(duration.as_millis()).map_err(|_| RelayError::Clock)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoopEnd {
    ClientClosed,
    PeerClosed(SessionCloseReason),
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthenticationError {
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("session is not registered")]
    UnknownSession,
    #[error("session credential has expired")]
    ExpiredSession,
    #[error("session token is invalid")]
    InvalidToken,
    #[error("session token belongs to the opposite role")]
    RoleMismatch,
    #[error("session token has already been used")]
    TokenAlreadyUsed,
}

impl AuthenticationError {
    const fn code(self) -> RelayProtocolErrorCode {
        match self {
            Self::UnsupportedVersion(_) => RelayProtocolErrorCode::UnsupportedVersion,
            Self::UnknownSession => RelayProtocolErrorCode::UnknownSession,
            Self::ExpiredSession => RelayProtocolErrorCode::ExpiredSession,
            Self::InvalidToken => RelayProtocolErrorCode::InvalidToken,
            Self::RoleMismatch => RelayProtocolErrorCode::RoleMismatch,
            Self::TokenAlreadyUsed => RelayProtocolErrorCode::TokenAlreadyUsed,
        }
    }
}

#[derive(Debug, Error)]
pub enum RelayError {
    #[error(transparent)]
    Authentication(#[from] AuthenticationError),
    #[error("the same role attempted to join a session twice")]
    DuplicateRole,
    #[error("relay capacity is exhausted")]
    CapacityExceeded,
    #[error("session is not ready for application payloads")]
    SessionNotReady,
    #[error("handshake timed out")]
    HandshakeTimeout,
    #[error("peer failed to acknowledge liveness before timeout")]
    PeerTimeout,
    #[error("peer outbound queue is full")]
    SlowConsumer,
    #[error("peer disconnected")]
    PeerDisconnected,
    #[error("invalid client message: {0}")]
    InvalidClientMessage(String),
    #[error("invalid server message: {0}")]
    InvalidServerMessage(String),
    #[error("system clock is before the Unix epoch or out of range")]
    Clock,
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Connection(#[from] quinn::ConnectionError),
    #[error(transparent)]
    ClosedStream(#[from] quinn::ClosedStream),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl RelayError {
    fn protocol_error(&self) -> Option<RelayServerMessage> {
        let (code, message) = match self {
            Self::Authentication(error) => (error.code(), error.to_string()),
            Self::DuplicateRole => (RelayProtocolErrorCode::DuplicateRole, self.to_string()),
            Self::CapacityExceeded => (RelayProtocolErrorCode::CapacityExceeded, self.to_string()),
            Self::SessionNotReady => (RelayProtocolErrorCode::SessionNotReady, self.to_string()),
            Self::InvalidClientMessage(_) => {
                (RelayProtocolErrorCode::MalformedMessage, self.to_string())
            }
            Self::Transport(TransportError::FrameTooLarge { .. }) => {
                (RelayProtocolErrorCode::FrameTooLarge, self.to_string())
            }
            _ => return None,
        };
        Some(RelayServerMessage::ProtocolError { code, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn credentials_are_role_bound_and_one_time() {
        let authenticator = InMemorySessionAuthenticator::default();
        let session_id = SessionId::new();
        let controller_token = SessionToken::from_bytes([7; 32]);
        let agent_token = SessionToken::from_bytes([8; 32]);
        authenticator
            .grant(session_id, Role::Controller, controller_token.clone(), 200)
            .await;
        authenticator
            .grant(session_id, Role::Agent, agent_token, 200)
            .await;

        let wrong_role = ClientHello::new(session_id, Role::Agent, controller_token.clone());
        assert_eq!(
            authenticator.authenticate(&wrong_role, 100).await,
            Err(AuthenticationError::RoleMismatch)
        );
        let valid = ClientHello::new(session_id, Role::Controller, controller_token);
        authenticator
            .authenticate(&valid, 100)
            .await
            .expect("valid grant");
        assert_eq!(
            authenticator.authenticate(&valid, 100).await,
            Err(AuthenticationError::TokenAlreadyUsed)
        );
    }

    #[tokio::test]
    async fn expired_credentials_are_rejected() {
        let authenticator = InMemorySessionAuthenticator::default();
        let session_id = SessionId::new();
        let token = SessionToken::from_bytes([9; 32]);
        authenticator
            .grant(session_id, Role::Controller, token.clone(), 99)
            .await;
        let hello = ClientHello::new(session_id, Role::Controller, token);

        assert_eq!(
            authenticator.authenticate(&hello, 100).await,
            Err(AuthenticationError::ExpiredSession)
        );
    }
}
