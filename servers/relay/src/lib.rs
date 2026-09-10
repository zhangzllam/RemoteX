//! Authenticated QUIC session pairing, liveness, and opaque payload forwarding.

use async_trait::async_trait;
use quinn::{Endpoint, RecvStream, SendStream};
use remotex_protocol::{
    ClientHello, MAX_RELAY_HANDSHAKE_SIZE, PROTOCOL_VERSION, RelayClientMessage,
    RelayCredentialKind, RelayProtocolErrorCode, RelayServerMessage, Role, SessionCloseReason,
    SessionId, SessionToken, decode_wire, encode_wire,
};
use remotex_transport::{TransportError, read_frame, write_frame};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
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
    recovery_token: Option<SessionToken>,
    recovery_expires_at_ms: u64,
    recovery_enabled: bool,
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
            recovery_token: None,
            recovery_expires_at_ms: 0,
            recovery_enabled: false,
        });
        match role {
            Role::Controller => session.controller = grant,
            Role::Agent => session.agent = grant,
        }
    }

    pub async fn grant_recovery(
        &self,
        session_id: SessionId,
        role: Role,
        token: SessionToken,
        expires_at_ms: u64,
    ) -> Result<(), AuthenticationError> {
        let mut sessions = self.sessions.lock().await;
        let credential = sessions
            .get_mut(&session_id)
            .and_then(|session| session.credential_mut(role))
            .ok_or(AuthenticationError::UnknownSession)?;
        credential.recovery_token = Some(token);
        credential.recovery_expires_at_ms = expires_at_ms;
        credential.recovery_enabled = true;
        Ok(())
    }

    pub async fn revoke_recovery(&self, session_id: SessionId) {
        if let Some(session) = self.sessions.lock().await.get_mut(&session_id) {
            for role in [Role::Controller, Role::Agent] {
                if let Some(credential) = session.credential_mut(role) {
                    credential.recovery_enabled = false;
                }
            }
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
            .is_some_and(|grant| {
                let token = match hello.credential_kind {
                    RelayCredentialKind::Initial => Some(&grant.token),
                    RelayCredentialKind::Recovery => grant.recovery_token.as_ref(),
                };
                token
                    .is_some_and(|token| bool::from(token.as_bytes().ct_eq(hello.token.as_bytes())))
            })
        {
            return Err(AuthenticationError::RoleMismatch);
        }
        let credential = session
            .credential_mut(hello.role)
            .ok_or(AuthenticationError::InvalidToken)?;
        match hello.credential_kind {
            RelayCredentialKind::Initial => {
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
            }
            RelayCredentialKind::Recovery => {
                if !credential.recovery_enabled || now_ms > credential.recovery_expires_at_ms {
                    return Err(AuthenticationError::ExpiredSession);
                }
                let expected = credential
                    .recovery_token
                    .as_ref()
                    .ok_or(AuthenticationError::InvalidToken)?;
                if !bool::from(expected.as_bytes().ct_eq(hello.token.as_bytes())) {
                    return Err(AuthenticationError::InvalidToken);
                }
            }
        }
        Ok(AuthenticatedSession {
            session_id: hello.session_id,
            role: hello.role,
        })
    }
}

/// Compatibility name used by the existing composition root.
pub type SessionAuthorizer = InMemorySessionAuthenticator;

/// PostgreSQL-backed one-time credentials issued by the M8 control server.
#[derive(Clone)]
pub struct PostgresSessionAuthenticator {
    pool: PgPool,
}

impl PostgresSessionAuthenticator {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl SessionAuthenticator for PostgresSessionAuthenticator {
    async fn authenticate(
        &self,
        hello: &ClientHello,
        now_ms: u64,
    ) -> Result<AuthenticatedSession, AuthenticationError> {
        if hello.version != PROTOCOL_VERSION {
            return Err(AuthenticationError::UnsupportedVersion(hello.version));
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthenticationError::BackendUnavailable)?;
        let row = sqlx::query(
            "SELECT controller_token_hash, agent_token_hash, controller_consumed, agent_consumed, expires_at_ms, controller_recovery_token_hash, agent_recovery_token_hash, recovery_expires_at_ms, authorization_status FROM sessions WHERE session_id=$1::uuid FOR UPDATE",
        )
        .bind(hello.session_id.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| AuthenticationError::BackendUnavailable)?
        .ok_or(AuthenticationError::UnknownSession)?;
        let controller_hash: Vec<u8> = row
            .try_get("controller_token_hash")
            .map_err(|_| AuthenticationError::BackendUnavailable)?;
        let agent_hash: Vec<u8> = row
            .try_get("agent_token_hash")
            .map_err(|_| AuthenticationError::BackendUnavailable)?;
        let presented: [u8; 32] = Sha256::digest(hello.token.as_bytes()).into();
        let (initial_expected, initial_opposite, consumed, consumed_column) = match hello.role {
            Role::Controller => (
                controller_hash.as_slice(),
                agent_hash.as_slice(),
                row.try_get::<bool, _>("controller_consumed")
                    .map_err(|_| AuthenticationError::BackendUnavailable)?,
                "controller_consumed",
            ),
            Role::Agent => (
                agent_hash.as_slice(),
                controller_hash.as_slice(),
                row.try_get::<bool, _>("agent_consumed")
                    .map_err(|_| AuthenticationError::BackendUnavailable)?,
                "agent_consumed",
            ),
        };
        let (expected, opposite, expires_at_ms, one_time) = match hello.credential_kind {
            RelayCredentialKind::Initial => {
                let expires_at_ms: i64 = row
                    .try_get("expires_at_ms")
                    .map_err(|_| AuthenticationError::BackendUnavailable)?;
                (
                    initial_expected.to_vec(),
                    initial_opposite.to_vec(),
                    expires_at_ms,
                    true,
                )
            }
            RelayCredentialKind::Recovery => {
                let status: String = row
                    .try_get("authorization_status")
                    .map_err(|_| AuthenticationError::BackendUnavailable)?;
                if status != "accepted" {
                    return Err(AuthenticationError::InvalidToken);
                }
                let controller: Vec<u8> = row
                    .try_get("controller_recovery_token_hash")
                    .map_err(|_| AuthenticationError::BackendUnavailable)?;
                let agent: Vec<u8> = row
                    .try_get("agent_recovery_token_hash")
                    .map_err(|_| AuthenticationError::BackendUnavailable)?;
                let (expected, opposite) = match hello.role {
                    Role::Controller => (controller, agent),
                    Role::Agent => (agent, controller),
                };
                let expires_at_ms: i64 = row
                    .try_get("recovery_expires_at_ms")
                    .map_err(|_| AuthenticationError::BackendUnavailable)?;
                (expected, opposite, expires_at_ms, false)
            }
        };
        if bool::from(opposite.as_slice().ct_eq(&presented)) {
            return Err(AuthenticationError::RoleMismatch);
        }
        if !bool::from(expected.as_slice().ct_eq(&presented)) {
            return Err(AuthenticationError::InvalidToken);
        }
        let expires_at_ms =
            u64::try_from(expires_at_ms).map_err(|_| AuthenticationError::BackendUnavailable)?;
        if now_ms > expires_at_ms {
            return Err(AuthenticationError::ExpiredSession);
        }
        if one_time && consumed {
            return Err(AuthenticationError::TokenAlreadyUsed);
        }
        if one_time {
            let statement =
                format!("UPDATE sessions SET {consumed_column}=TRUE WHERE session_id=$1::uuid");
            sqlx::query(&statement)
                .bind(hello.session_id.to_string())
                .execute(&mut *transaction)
                .await
                .map_err(|_| AuthenticationError::BackendUnavailable)?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| AuthenticationError::BackendUnavailable)?;
        Ok(AuthenticatedSession {
            session_id: hello.session_id,
            role: hello.role,
        })
    }
}

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

#[derive(Default)]
struct RelayCounters {
    authentication_failures: AtomicU64,
    bytes_forwarded: AtomicU64,
    direct_upgrades: AtomicU64,
    peer_timeouts: AtomicU64,
    slow_consumer_disconnects: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RelayMetricsSnapshot {
    pub active_sessions: u64,
    pub waiting_sessions: u64,
    pub authentication_failures: u64,
    pub bytes_forwarded: u64,
    pub direct_upgrades: u64,
    pub peer_timeouts: u64,
    pub slow_consumer_disconnects: u64,
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

    async fn forward_path_control(
        &self,
        session_id: SessionId,
        source_role: Role,
        message: remotex_protocol::PathControlMessage,
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
        queue_message(&target, RelayServerMessage::PathControl(message))
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
    counters: Arc<RelayCounters>,
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
            counters: Arc::new(RelayCounters::default()),
        }
    }

    #[must_use]
    pub fn with_authenticator(
        authenticator: Arc<dyn SessionAuthenticator>,
        limits: RelayLimits,
    ) -> Self {
        Self {
            authenticator,
            registry: Arc::new(SessionRegistry::default()),
            connection_slots: Arc::new(Semaphore::new(limits.max_connections)),
            connection_sequence: Arc::new(AtomicU64::new(1)),
            limits,
            counters: Arc::new(RelayCounters::default()),
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

    pub async fn metrics_snapshot(&self) -> RelayMetricsSnapshot {
        let sessions = self.registry.sessions.lock().await;
        let active_sessions = sessions.values().filter(|session| session.ready()).count() as u64;
        let waiting_sessions = sessions.len() as u64 - active_sessions;
        RelayMetricsSnapshot {
            active_sessions,
            waiting_sessions,
            authentication_failures: self
                .counters
                .authentication_failures
                .load(Ordering::Relaxed),
            bytes_forwarded: self.counters.bytes_forwarded.load(Ordering::Relaxed),
            direct_upgrades: self.counters.direct_upgrades.load(Ordering::Relaxed),
            peer_timeouts: self.counters.peer_timeouts.load(Ordering::Relaxed),
            slow_consumer_disconnects: self
                .counters
                .slow_consumer_disconnects
                .load(Ordering::Relaxed),
        }
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
        info!(event = "connection_received", connection_id);
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
                self.counters
                    .authentication_failures
                    .fetch_add(1, Ordering::Relaxed);
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
        match peer_reason {
            SessionCloseReason::HeartbeatTimeout => {
                self.counters.peer_timeouts.fetch_add(1, Ordering::Relaxed);
            }
            SessionCloseReason::SlowConsumer => {
                self.counters
                    .slow_consumer_disconnects
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
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
                            self.counters.bytes_forwarded.fetch_add(payload.len() as u64, Ordering::Relaxed);
                            self.registry
                                .forward(authenticated.session_id, authenticated.role, payload)
                                .await?;
                        }
                        RelayClientMessage::Heartbeat { nonce } => {
                            queue_message(outbound, RelayServerMessage::HeartbeatAck { nonce })?;
                        }
                        RelayClientMessage::HeartbeatAck { .. } => {}
                        RelayClientMessage::PathControl(message) => {
                            if matches!(message, remotex_protocol::PathControlMessage::SwitchCommitted { .. }) {
                                self.counters.direct_upgrades.fetch_add(1, Ordering::Relaxed);
                            }
                            self.registry
                                .forward_path_control(
                                    authenticated.session_id,
                                    authenticated.role,
                                    message,
                                )
                                .await?;
                        }
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
    #[error("session authentication backend is unavailable")]
    BackendUnavailable,
}

impl AuthenticationError {
    const fn code(self) -> RelayProtocolErrorCode {
        match self {
            Self::UnsupportedVersion(_) => RelayProtocolErrorCode::UnsupportedVersion,
            Self::UnknownSession | Self::BackendUnavailable => {
                RelayProtocolErrorCode::UnknownSession
            }
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

    #[tokio::test]
    async fn recovery_credentials_are_role_bound_expiring_and_revocable() {
        let authenticator = InMemorySessionAuthenticator::default();
        let session_id = SessionId::new();
        let controller_initial = SessionToken::from_bytes([1; 32]);
        let agent_initial = SessionToken::from_bytes([2; 32]);
        let controller_recovery = SessionToken::from_bytes([3; 32]);
        let agent_recovery = SessionToken::from_bytes([4; 32]);
        authenticator
            .grant(session_id, Role::Controller, controller_initial, 200)
            .await;
        authenticator
            .grant(session_id, Role::Agent, agent_initial, 200)
            .await;
        authenticator
            .grant_recovery(
                session_id,
                Role::Controller,
                controller_recovery.clone(),
                500,
            )
            .await
            .expect("grant controller recovery");
        authenticator
            .grant_recovery(session_id, Role::Agent, agent_recovery, 500)
            .await
            .expect("grant agent recovery");

        let wrong_role =
            ClientHello::recovery(session_id, Role::Agent, controller_recovery.clone());
        assert_eq!(
            authenticator.authenticate(&wrong_role, 300).await,
            Err(AuthenticationError::RoleMismatch)
        );
        let valid =
            ClientHello::recovery(session_id, Role::Controller, controller_recovery.clone());
        authenticator
            .authenticate(&valid, 300)
            .await
            .expect("valid recovery credential");
        assert_eq!(
            authenticator.authenticate(&valid, 501).await,
            Err(AuthenticationError::ExpiredSession)
        );

        authenticator.revoke_recovery(session_id).await;
        assert_eq!(
            authenticator.authenticate(&valid, 400).await,
            Err(AuthenticationError::ExpiredSession)
        );
    }

    #[tokio::test]
    async fn registry_capacity_remains_bounded_for_many_idle_sessions() {
        let registry = SessionRegistry::default();
        for index in 0..1_000_u64 {
            let session_id = SessionId::new();
            let (outbound, _receiver) = mpsc::channel(1);
            let (shutdown, _shutdown_receiver) = watch::channel(None);
            let result = registry
                .register(
                    session_id,
                    Role::Controller,
                    PeerHandle {
                        connection_id: index,
                        outbound,
                        shutdown,
                    },
                    500,
                )
                .await;
            if index < 500 {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(RelayError::CapacityExceeded)));
            }
        }
        assert_eq!(registry.session_count().await, 500);
    }
}
