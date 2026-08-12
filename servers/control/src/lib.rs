//! M8 control plane: device registration, presence, and short-lived sessions.

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::Rng;
use remotex_crypto::{ServerSecretBox, device_auth_message, secret_hash, verify_device_signature};
use remotex_protocol::{
    AuthorizationDecision, ClaimAgentSessionRequest, ClaimAgentSessionResponse, ConnectionType,
    CreateSessionRequest, DeviceAuthProof, DeviceHeartbeatRequest, DeviceId, DevicePlatform,
    DeviceRecord, DeviceRegistrationRequest, DeviceRegistrationResponse, IncomingSessionRequest,
    ReportSessionEventRequest, ResolveSessionAuthorizationRequest,
    ResolveSessionAuthorizationResponse, SessionAuditEventKind, SessionCredentials, SessionId,
    SessionPermissions,
};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::{collections::HashMap, sync::Arc};
use thiserror::Error;
use tokio::sync::Mutex;

pub const DEFAULT_DEVICE_OFFLINE_AFTER_MS: u64 = 45_000;
pub const DEFAULT_SESSION_LIFETIME_MS: u64 = 5 * 60 * 1_000;
pub const DEVICE_AUTH_CLOCK_SKEW_MS: u64 = 60_000;

#[derive(Clone, Debug)]
pub struct ControlConfig {
    pub relay_address: String,
    pub relay_server_name: String,
    pub device_offline_after_ms: u64,
    pub session_lifetime_ms: u64,
}

#[derive(Clone, Debug)]
pub struct StoredDevice {
    pub record: DeviceRecord,
    pub public_key: [u8; 32],
    pub last_auth_nonce: u64,
}

#[derive(Clone, Debug)]
pub struct NewSession {
    pub session_id: SessionId,
    pub device_id: DeviceId,
    pub controller_name: String,
    pub permissions: SessionPermissions,
    pub controller_token_hash: [u8; 32],
    pub agent_token_hash: [u8; 32],
    pub agent_token_wrapped: Vec<u8>,
    pub e2e_key_wrapped: Vec<u8>,
    pub unattended_secret_wrapped: Option<Vec<u8>>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct ClaimableSession {
    pub session_id: SessionId,
    pub controller_name: String,
    pub permissions: SessionPermissions,
    pub agent_token_wrapped: Vec<u8>,
    pub e2e_key_wrapped: Vec<u8>,
    pub unattended_secret_wrapped: Option<Vec<u8>>,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct AuditEvent {
    pub session_id: SessionId,
    pub device_id: DeviceId,
    pub event_type: &'static str,
    pub occurred_at_ms: u64,
    pub metadata: serde_json::Value,
}

#[async_trait]
pub trait ControlRepository: Send + Sync {
    async fn register_device(
        &self,
        candidate_id: DeviceId,
        request: &DeviceRegistrationRequest,
        now_ms: u64,
    ) -> Result<StoredDevice, ControlError>;
    async fn get_device(&self, device_id: &DeviceId) -> Result<Option<StoredDevice>, ControlError>;
    async fn heartbeat(
        &self,
        device_id: &DeviceId,
        request: &DeviceHeartbeatRequest,
        now_ms: u64,
    ) -> Result<(), ControlError>;
    async fn advance_nonce(&self, device_id: &DeviceId, nonce: u64) -> Result<bool, ControlError>;
    async fn create_session(&self, session: NewSession) -> Result<(), ControlError>;
    async fn pending_session(
        &self,
        device_id: &DeviceId,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError>;
    async fn resolve_authorization(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        granted_permissions: Option<SessionPermissions>,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError>;
    async fn record_session_event(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        event: &ReportSessionEventRequest,
        now_ms: u64,
    ) -> Result<(), ControlError>;
    async fn record_audit(&self, event: AuditEvent) -> Result<(), ControlError>;
}

#[derive(Clone)]
pub struct ControlService {
    repository: Arc<dyn ControlRepository>,
    secrets: Arc<ServerSecretBox>,
    config: ControlConfig,
}

impl ControlService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn ControlRepository>,
        master_key: [u8; 32],
        config: ControlConfig,
    ) -> Self {
        Self {
            repository,
            secrets: Arc::new(ServerSecretBox::new(master_key)),
            config,
        }
    }

    pub async fn register(
        &self,
        request: DeviceRegistrationRequest,
        now_ms: u64,
    ) -> Result<DeviceRegistrationResponse, ControlError> {
        validate_registration(&request)?;
        let candidate_id = random_device_id()?;
        let device = self
            .repository
            .register_device(candidate_id, &request, now_ms)
            .await?;
        Ok(DeviceRegistrationResponse {
            device_id: device.record.device_id,
        })
    }

    pub async fn device(
        &self,
        device_id: &DeviceId,
        now_ms: u64,
    ) -> Result<DeviceRecord, ControlError> {
        let mut record = self
            .repository
            .get_device(device_id)
            .await?
            .ok_or(ControlError::DeviceNotFound)?
            .record;
        record.online =
            now_ms.saturating_sub(record.last_seen_ms) <= self.config.device_offline_after_ms;
        Ok(record)
    }

    pub async fn heartbeat(
        &self,
        device_id: &DeviceId,
        request: DeviceHeartbeatRequest,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        self.authenticate_device(device_id, "heartbeat", &request.proof, now_ms)
            .await?;
        self.repository.heartbeat(device_id, &request, now_ms).await
    }

    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
        now_ms: u64,
    ) -> Result<SessionCredentials, ControlError> {
        validate_name(&request.controller_name)?;
        if request
            .unattended_secret
            .as_ref()
            .is_some_and(|secret| !(12..=128).contains(&secret.len()))
        {
            return Err(ControlError::InvalidSecret);
        }
        let device = self
            .repository
            .get_device(&request.device_id)
            .await?
            .ok_or(ControlError::DeviceNotFound)?;
        if now_ms.saturating_sub(device.record.last_seen_ms) > self.config.device_offline_after_ms {
            return Err(ControlError::DeviceOffline);
        }
        let permissions = request
            .requested_permissions
            .intersect(device.record.capabilities);
        let controller_token: [u8; 32] = rand::random();
        let agent_token: [u8; 32] = rand::random();
        let end_to_end_key: [u8; 32] = rand::random();
        let session_id = SessionId::new();
        let expires_at_ms = now_ms
            .checked_add(self.config.session_lifetime_ms)
            .ok_or(ControlError::ClockOverflow)?;
        let new_session = NewSession {
            session_id,
            device_id: request.device_id.clone(),
            controller_name: request.controller_name.clone(),
            permissions,
            controller_token_hash: secret_hash(&controller_token),
            agent_token_hash: secret_hash(&agent_token),
            agent_token_wrapped: self.secrets.seal(&agent_token)?,
            e2e_key_wrapped: self.secrets.seal(&end_to_end_key)?,
            unattended_secret_wrapped: request
                .unattended_secret
                .as_ref()
                .map(|secret| self.secrets.seal(secret.as_bytes()))
                .transpose()?,
            created_at_ms: now_ms,
            expires_at_ms,
        };
        self.repository.create_session(new_session).await?;
        self.repository
            .record_audit(AuditEvent {
                session_id,
                device_id: request.device_id,
                event_type: "session_requested",
                occurred_at_ms: now_ms,
                metadata: serde_json::json!({
                    "controller_name": request.controller_name,
                    "permissions": permissions,
                }),
            })
            .await?;
        Ok(self.credentials(
            session_id,
            controller_token,
            end_to_end_key,
            expires_at_ms,
            permissions,
        ))
    }

    pub async fn claim_agent_session(
        &self,
        device_id: &DeviceId,
        request: ClaimAgentSessionRequest,
        now_ms: u64,
    ) -> Result<ClaimAgentSessionResponse, ControlError> {
        self.authenticate_device(device_id, "claim_session", &request.proof, now_ms)
            .await?;
        let Some(session) = self.repository.pending_session(device_id, now_ms).await? else {
            return Ok(ClaimAgentSessionResponse {
                authorization_request: None,
            });
        };
        Ok(ClaimAgentSessionResponse {
            authorization_request: Some(IncomingSessionRequest {
                session_id: session.session_id,
                controller_name: session.controller_name,
                requested_permissions: session.permissions,
                expires_at_ms: session.expires_at_ms,
                unattended_secret: session
                    .unattended_secret_wrapped
                    .map(|wrapped| self.secrets.open(&wrapped))
                    .transpose()?
                    .map(|secret| String::from_utf8(secret).map_err(|_| ControlError::Crypto))
                    .transpose()?,
            }),
        })
    }

    pub async fn resolve_session_authorization(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        request: ResolveSessionAuthorizationRequest,
        now_ms: u64,
    ) -> Result<ResolveSessionAuthorizationResponse, ControlError> {
        let action = format!("authorize_session:{session_id}");
        self.authenticate_device(device_id, &action, &request.proof, now_ms)
            .await?;
        let pending = self
            .repository
            .pending_session(device_id, now_ms)
            .await?
            .filter(|session| session.session_id == session_id)
            .ok_or(ControlError::Conflict)?;
        let granted_permissions = match request.decision {
            AuthorizationDecision::Accept => {
                Some(request.granted_permissions.intersect(pending.permissions))
            }
            AuthorizationDecision::Reject => None,
        };
        let resolved = self
            .repository
            .resolve_authorization(device_id, session_id, granted_permissions, now_ms)
            .await?
            .ok_or(ControlError::Conflict)?;
        let accepted = granted_permissions.is_some();
        self.repository
            .record_audit(AuditEvent {
                session_id,
                device_id: device_id.clone(),
                event_type: if accepted {
                    "session_accepted"
                } else {
                    "session_rejected"
                },
                occurred_at_ms: now_ms,
                metadata: serde_json::json!({ "permissions": granted_permissions }),
            })
            .await?;
        let credentials = if let Some(permissions) = granted_permissions {
            let agent_token = fixed_secret(self.secrets.open(&resolved.agent_token_wrapped)?)?;
            let e2e_key = fixed_secret(self.secrets.open(&resolved.e2e_key_wrapped)?)?;
            Some(self.credentials(
                session_id,
                agent_token,
                e2e_key,
                resolved.expires_at_ms,
                permissions,
            ))
        } else {
            None
        };
        Ok(ResolveSessionAuthorizationResponse { credentials })
    }

    pub async fn report_session_event(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        request: ReportSessionEventRequest,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        if request.result.len() > 64 || request.result.chars().any(char::is_control) {
            return Err(ControlError::InvalidResult);
        }
        let action = format!(
            "session_event:{session_id}:{}",
            audit_kind_text(request.kind)
        );
        self.authenticate_device(device_id, &action, &request.proof, now_ms)
            .await?;
        self.repository
            .record_session_event(device_id, session_id, &request, now_ms)
            .await?;
        let event_type = match request.kind {
            SessionAuditEventKind::Started => "session_started",
            SessionAuditEventKind::Ended => "session_ended",
        };
        self.repository
            .record_audit(AuditEvent {
                session_id,
                device_id: device_id.clone(),
                event_type,
                occurred_at_ms: now_ms,
                metadata: serde_json::json!({
                    "connection_type": request.connection_type,
                    "bytes_transferred": request.bytes_transferred,
                    "result": request.result,
                }),
            })
            .await
    }

    async fn authenticate_device(
        &self,
        device_id: &DeviceId,
        action: &str,
        proof: &DeviceAuthProof,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        if proof.timestamp_ms.abs_diff(now_ms) > DEVICE_AUTH_CLOCK_SKEW_MS {
            return Err(ControlError::StaleProof);
        }
        let device = self
            .repository
            .get_device(device_id)
            .await?
            .ok_or(ControlError::DeviceNotFound)?;
        verify_device_signature(
            &device.public_key,
            &device_auth_message(action, device_id.as_str(), proof.timestamp_ms, proof.nonce),
            &proof.signature,
        )?;
        if proof.nonce <= device.last_auth_nonce
            || !self
                .repository
                .advance_nonce(device_id, proof.nonce)
                .await?
        {
            return Err(ControlError::ReplayedProof);
        }
        Ok(())
    }

    fn credentials(
        &self,
        session_id: SessionId,
        role_token: [u8; 32],
        end_to_end_key: [u8; 32],
        expires_at_ms: u64,
        permissions: SessionPermissions,
    ) -> SessionCredentials {
        SessionCredentials {
            session_id,
            relay_address: self.config.relay_address.clone(),
            relay_server_name: self.config.relay_server_name.clone(),
            role_token_hex: hex::encode(role_token),
            end_to_end_key_hex: hex::encode(end_to_end_key),
            expires_at_ms,
            permissions,
        }
    }
}

#[derive(Clone)]
pub struct ApiState {
    pub service: ControlService,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(health))
        .route("/api/devices/register", post(register_device))
        .route("/api/devices/{device_id}", get(get_device))
        .route("/api/devices/{device_id}/heartbeat", post(heartbeat))
        .route(
            "/api/devices/{device_id}/sessions/claim",
            post(claim_agent_session),
        )
        .route(
            "/api/devices/{device_id}/sessions/{session_id}/authorize",
            post(resolve_session_authorization),
        )
        .route(
            "/api/devices/{device_id}/sessions/{session_id}/events",
            post(report_session_event),
        )
        .route("/api/sessions", post(create_session))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn register_device(
    State(state): State<ApiState>,
    Json(request): Json<DeviceRegistrationRequest>,
) -> Result<(StatusCode, Json<DeviceRegistrationResponse>), ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(state.service.register(request, now_ms()?).await?),
    ))
}

async fn get_device(
    State(state): State<ApiState>,
    Path(device_id): Path<String>,
) -> Result<Json<DeviceRecord>, ControlError> {
    let device_id = DeviceId::new(device_id).map_err(|_| ControlError::InvalidDeviceId)?;
    Ok(Json(state.service.device(&device_id, now_ms()?).await?))
}

async fn heartbeat(
    State(state): State<ApiState>,
    Path(device_id): Path<String>,
    Json(request): Json<DeviceHeartbeatRequest>,
) -> Result<StatusCode, ControlError> {
    let device_id = DeviceId::new(device_id).map_err(|_| ControlError::InvalidDeviceId)?;
    state
        .service
        .heartbeat(&device_id, request, now_ms()?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_session(
    State(state): State<ApiState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionCredentials>), ControlError> {
    Ok((
        StatusCode::CREATED,
        Json(state.service.create_session(request, now_ms()?).await?),
    ))
}

async fn claim_agent_session(
    State(state): State<ApiState>,
    Path(device_id): Path<String>,
    Json(request): Json<ClaimAgentSessionRequest>,
) -> Result<Json<ClaimAgentSessionResponse>, ControlError> {
    let device_id = DeviceId::new(device_id).map_err(|_| ControlError::InvalidDeviceId)?;
    Ok(Json(
        state
            .service
            .claim_agent_session(&device_id, request, now_ms()?)
            .await?,
    ))
}

async fn resolve_session_authorization(
    State(state): State<ApiState>,
    Path((device_id, session_id)): Path<(String, String)>,
    Json(request): Json<ResolveSessionAuthorizationRequest>,
) -> Result<Json<ResolveSessionAuthorizationResponse>, ControlError> {
    let device_id = DeviceId::new(device_id).map_err(|_| ControlError::InvalidDeviceId)?;
    let session_id = session_id
        .parse()
        .map_err(|_| ControlError::InvalidSessionId)?;
    Ok(Json(
        state
            .service
            .resolve_session_authorization(&device_id, session_id, request, now_ms()?)
            .await?,
    ))
}

async fn report_session_event(
    State(state): State<ApiState>,
    Path((device_id, session_id)): Path<(String, String)>,
    Json(request): Json<ReportSessionEventRequest>,
) -> Result<StatusCode, ControlError> {
    let device_id = DeviceId::new(device_id).map_err(|_| ControlError::InvalidDeviceId)?;
    let session_id = session_id
        .parse()
        .map_err(|_| ControlError::InvalidSessionId)?;
    state
        .service
        .report_session_event(&device_id, session_id, request, now_ms()?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct ErrorResponse {
    code: &'static str,
    message: String,
}

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::InvalidDeviceId
            | Self::InvalidSessionId
            | Self::InvalidRegistration
            | Self::InvalidName
            | Self::InvalidResult
            | Self::InvalidSecret => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::DeviceNotFound => (StatusCode::NOT_FOUND, "device_not_found"),
            Self::DeviceOffline => (StatusCode::CONFLICT, "device_offline"),
            Self::StaleProof | Self::ReplayedProof | Self::Authentication => {
                (StatusCode::UNAUTHORIZED, "device_authentication_failed")
            }
            Self::Conflict => (StatusCode::CONFLICT, "conflict"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        (
            status,
            Json(ErrorResponse {
                code,
                message: self.public_message().to_owned(),
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Error)]
pub enum ControlError {
    #[error("device ID is invalid")]
    InvalidDeviceId,
    #[error("session ID is invalid")]
    InvalidSessionId,
    #[error("device registration is invalid")]
    InvalidRegistration,
    #[error("name is invalid")]
    InvalidName,
    #[error("session result is invalid")]
    InvalidResult,
    #[error("unattended-access secret is invalid")]
    InvalidSecret,
    #[error("device was not found")]
    DeviceNotFound,
    #[error("device is offline")]
    DeviceOffline,
    #[error("device proof is outside the accepted clock window")]
    StaleProof,
    #[error("device proof nonce was already used")]
    ReplayedProof,
    #[error("device authentication failed")]
    Authentication,
    #[error("resource already exists")]
    Conflict,
    #[error("clock value overflowed")]
    ClockOverflow,
    #[error("persistence operation failed")]
    Persistence,
    #[error("cryptographic operation failed")]
    Crypto,
}

impl ControlError {
    const fn public_message(&self) -> &'static str {
        match self {
            Self::InvalidDeviceId => "device ID is invalid",
            Self::InvalidSessionId => "session ID is invalid",
            Self::InvalidRegistration => "device registration is invalid",
            Self::InvalidName => "name is invalid",
            Self::InvalidResult => "session result is invalid",
            Self::InvalidSecret => "unattended-access secret is invalid",
            Self::DeviceNotFound => "device was not found",
            Self::DeviceOffline => "device is offline",
            Self::StaleProof | Self::ReplayedProof | Self::Authentication => {
                "device authentication failed"
            }
            Self::Conflict => "resource already exists",
            Self::ClockOverflow | Self::Persistence | Self::Crypto => "internal server error",
        }
    }
}

impl From<remotex_crypto::CryptoError> for ControlError {
    fn from(_: remotex_crypto::CryptoError) -> Self {
        Self::Authentication
    }
}

#[derive(Clone)]
pub struct PostgresRepository {
    pool: PgPool,
}

impl PostgresRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ControlRepository for PostgresRepository {
    async fn register_device(
        &self,
        candidate_id: DeviceId,
        request: &DeviceRegistrationRequest,
        now_ms: u64,
    ) -> Result<StoredDevice, ControlError> {
        let capabilities =
            serde_json::to_string(&request.capabilities).map_err(|_| ControlError::Persistence)?;
        let now = i64_value(now_ms)?;
        sqlx::query(
            "INSERT INTO devices (device_id, public_key, device_name, platform, agent_version, capabilities_json, registered_at_ms, last_seen_ms) VALUES ($1,$2,$3,$4,$5,$6,$7,$7) ON CONFLICT (public_key) DO UPDATE SET device_name=EXCLUDED.device_name, platform=EXCLUDED.platform, agent_version=EXCLUDED.agent_version, capabilities_json=EXCLUDED.capabilities_json RETURNING device_id",
        )
        .bind(candidate_id.as_str())
        .bind(&request.public_key)
        .bind(&request.device_name)
        .bind(platform_text(request.platform))
        .bind(&request.agent_version)
        .bind(capabilities)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| ControlError::Persistence)?;
        let row = sqlx::query("SELECT device_id, public_key, device_name, platform, agent_version, capabilities_json, last_seen_ms, last_auth_nonce FROM devices WHERE public_key=$1")
            .bind(&request.public_key)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| ControlError::Persistence)?;
        stored_device_from_row(&row)
    }

    async fn get_device(&self, device_id: &DeviceId) -> Result<Option<StoredDevice>, ControlError> {
        let row = sqlx::query("SELECT device_id, public_key, device_name, platform, agent_version, capabilities_json, last_seen_ms, last_auth_nonce FROM devices WHERE device_id=$1")
            .bind(device_id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| ControlError::Persistence)?;
        row.as_ref().map(stored_device_from_row).transpose()
    }

    async fn heartbeat(
        &self,
        device_id: &DeviceId,
        request: &DeviceHeartbeatRequest,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        let capabilities =
            serde_json::to_string(&request.capabilities).map_err(|_| ControlError::Persistence)?;
        let result = sqlx::query("UPDATE devices SET last_seen_ms=$2, agent_version=$3, platform=$4, capabilities_json=$5 WHERE device_id=$1")
            .bind(device_id.as_str())
            .bind(i64_value(now_ms)?)
            .bind(&request.agent_version)
            .bind(platform_text(request.platform))
            .bind(capabilities)
            .execute(&self.pool)
            .await
            .map_err(|_| ControlError::Persistence)?;
        if result.rows_affected() == 0 {
            return Err(ControlError::DeviceNotFound);
        }
        Ok(())
    }

    async fn advance_nonce(&self, device_id: &DeviceId, nonce: u64) -> Result<bool, ControlError> {
        let result = sqlx::query(
            "UPDATE devices SET last_auth_nonce=$2 WHERE device_id=$1 AND last_auth_nonce < $2",
        )
        .bind(device_id.as_str())
        .bind(i64_value(nonce)?)
        .execute(&self.pool)
        .await
        .map_err(|_| ControlError::Persistence)?;
        Ok(result.rows_affected() == 1)
    }

    async fn create_session(&self, session: NewSession) -> Result<(), ControlError> {
        sqlx::query("INSERT INTO sessions (session_id, device_id, controller_name, permissions_json, controller_token_hash, agent_token_hash, agent_token_wrapped, e2e_key_wrapped, created_at_ms, expires_at_ms, unattended_secret_wrapped) VALUES ($1::uuid,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(session.session_id.to_string())
            .bind(session.device_id.as_str())
            .bind(session.controller_name)
            .bind(serde_json::to_string(&session.permissions).map_err(|_| ControlError::Persistence)?)
            .bind(session.controller_token_hash.as_slice())
            .bind(session.agent_token_hash.as_slice())
            .bind(session.agent_token_wrapped)
            .bind(session.e2e_key_wrapped)
            .bind(i64_value(session.created_at_ms)?)
            .bind(i64_value(session.expires_at_ms)?)
            .bind(session.unattended_secret_wrapped)
            .execute(&self.pool)
            .await
            .map_err(|_| ControlError::Persistence)?;
        Ok(())
    }

    async fn pending_session(
        &self,
        device_id: &DeviceId,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError> {
        let row = sqlx::query("SELECT session_id::text, controller_name, permissions_json, agent_token_wrapped, e2e_key_wrapped, expires_at_ms, unattended_secret_wrapped FROM sessions WHERE device_id=$1 AND authorization_status='pending' AND expires_at_ms >= $2 ORDER BY created_at_ms LIMIT 1")
            .bind(device_id.as_str())
            .bind(i64_value(now_ms)?)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| ControlError::Persistence)?;
        row.as_ref().map(claimable_session_from_row).transpose()
    }

    async fn resolve_authorization(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        granted_permissions: Option<SessionPermissions>,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| ControlError::Persistence)?;
        let row = sqlx::query("SELECT session_id::text, controller_name, permissions_json, agent_token_wrapped, e2e_key_wrapped, expires_at_ms, unattended_secret_wrapped FROM sessions WHERE session_id=$1::uuid AND device_id=$2 AND authorization_status='pending' AND expires_at_ms >= $3 FOR UPDATE")
            .bind(session_id.to_string())
            .bind(device_id.as_str())
            .bind(i64_value(now_ms)?)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| ControlError::Persistence)?;
        let Some(row) = row else {
            transaction
                .commit()
                .await
                .map_err(|_| ControlError::Persistence)?;
            return Ok(None);
        };
        let (status, serialized, claimed) = match granted_permissions {
            Some(permissions) => (
                "accepted",
                Some(serde_json::to_string(&permissions).map_err(|_| ControlError::Persistence)?),
                true,
            ),
            None => ("rejected", None, false),
        };
        sqlx::query("UPDATE sessions SET authorization_status=$2, authorized_permissions_json=$3, authorized_at_ms=$4, agent_claimed=$5 WHERE session_id=$1::uuid")
            .bind(session_id.to_string())
            .bind(status)
            .bind(serialized)
            .bind(i64_value(now_ms)?)
            .bind(claimed)
            .execute(&mut *transaction)
            .await
            .map_err(|_| ControlError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| ControlError::Persistence)?;
        Ok(Some(claimable_session_from_row(&row)?))
    }

    async fn record_session_event(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        event: &ReportSessionEventRequest,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        let connection_type = connection_type_text(event.connection_type);
        let result = match event.kind {
            SessionAuditEventKind::Started => sqlx::query("UPDATE sessions SET started_at_ms=$3, connection_type=$4, result=$5 WHERE session_id=$1::uuid AND device_id=$2 AND authorization_status='accepted'")
                .bind(session_id.to_string())
                .bind(device_id.as_str())
                .bind(i64_value(now_ms)?)
                .bind(connection_type)
                .bind(&event.result)
                .execute(&self.pool)
                .await,
            SessionAuditEventKind::Ended => sqlx::query("UPDATE sessions SET ended_at_ms=$3, connection_type=$4, bytes_transferred=$5, result=$6, authorization_status='ended' WHERE session_id=$1::uuid AND device_id=$2 AND authorization_status='accepted'")
                .bind(session_id.to_string())
                .bind(device_id.as_str())
                .bind(i64_value(now_ms)?)
                .bind(connection_type)
                .bind(i64_value(event.bytes_transferred)?)
                .bind(&event.result)
                .execute(&self.pool)
                .await,
        }
        .map_err(|_| ControlError::Persistence)?;
        if result.rows_affected() != 1 {
            return Err(ControlError::Conflict);
        }
        Ok(())
    }

    async fn record_audit(&self, event: AuditEvent) -> Result<(), ControlError> {
        sqlx::query("INSERT INTO audit_events (session_id, device_id, event_type, occurred_at_ms, metadata_json) VALUES ($1::uuid,$2,$3,$4,$5)")
            .bind(event.session_id.to_string())
            .bind(event.device_id.as_str())
            .bind(event.event_type)
            .bind(i64_value(event.occurred_at_ms)?)
            .bind(event.metadata.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| ControlError::Persistence)?;
        Ok(())
    }
}

fn claimable_session_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<ClaimableSession, ControlError> {
    Ok(ClaimableSession {
        session_id: row
            .try_get::<String, _>(0)
            .map_err(|_| ControlError::Persistence)?
            .parse()
            .map_err(|_| ControlError::Persistence)?,
        controller_name: row.try_get(1).map_err(|_| ControlError::Persistence)?,
        permissions: serde_json::from_str(
            row.try_get::<&str, _>(2)
                .map_err(|_| ControlError::Persistence)?,
        )
        .map_err(|_| ControlError::Persistence)?,
        agent_token_wrapped: row.try_get(3).map_err(|_| ControlError::Persistence)?,
        e2e_key_wrapped: row.try_get(4).map_err(|_| ControlError::Persistence)?,
        expires_at_ms: u64_value(row.try_get(5).map_err(|_| ControlError::Persistence)?)?,
        unattended_secret_wrapped: row.try_get(6).map_err(|_| ControlError::Persistence)?,
    })
}

fn stored_device_from_row(row: &sqlx::postgres::PgRow) -> Result<StoredDevice, ControlError> {
    let public_key: Vec<u8> = row
        .try_get("public_key")
        .map_err(|_| ControlError::Persistence)?;
    let public_key: [u8; 32] = public_key
        .try_into()
        .map_err(|_| ControlError::Persistence)?;
    let platform: String = row
        .try_get("platform")
        .map_err(|_| ControlError::Persistence)?;
    let capabilities: String = row
        .try_get("capabilities_json")
        .map_err(|_| ControlError::Persistence)?;
    Ok(StoredDevice {
        record: DeviceRecord {
            device_id: DeviceId::new(
                row.try_get::<String, _>("device_id")
                    .map_err(|_| ControlError::Persistence)?
                    .trim(),
            )
            .map_err(|_| ControlError::Persistence)?,
            device_name: row
                .try_get("device_name")
                .map_err(|_| ControlError::Persistence)?,
            platform: parse_platform(&platform)?,
            agent_version: row
                .try_get("agent_version")
                .map_err(|_| ControlError::Persistence)?,
            capabilities: serde_json::from_str(&capabilities)
                .map_err(|_| ControlError::Persistence)?,
            last_seen_ms: u64_value(
                row.try_get("last_seen_ms")
                    .map_err(|_| ControlError::Persistence)?,
            )?,
            online: false,
        },
        public_key,
        last_auth_nonce: u64_value(
            row.try_get("last_auth_nonce")
                .map_err(|_| ControlError::Persistence)?,
        )?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemoryAuthorization {
    Pending,
    Accepted,
    Rejected,
    Ended,
}

struct MemorySession {
    session: NewSession,
    authorization: MemoryAuthorization,
}

#[derive(Default)]
pub struct MemoryRepository {
    devices: Mutex<HashMap<DeviceId, StoredDevice>>,
    public_keys: Mutex<HashMap<[u8; 32], DeviceId>>,
    sessions: Mutex<Vec<MemorySession>>,
    audit_events: Mutex<Vec<AuditEvent>>,
}

impl MemoryRepository {
    pub async fn audit_event_count(&self) -> usize {
        self.audit_events.lock().await.len()
    }
}

#[async_trait]
impl ControlRepository for MemoryRepository {
    async fn register_device(
        &self,
        candidate_id: DeviceId,
        request: &DeviceRegistrationRequest,
        now_ms: u64,
    ) -> Result<StoredDevice, ControlError> {
        let public_key: [u8; 32] = request
            .public_key
            .clone()
            .try_into()
            .map_err(|_| ControlError::InvalidRegistration)?;
        if let Some(existing_id) = self.public_keys.lock().await.get(&public_key).cloned() {
            let mut devices = self.devices.lock().await;
            let device = devices
                .get_mut(&existing_id)
                .ok_or(ControlError::Persistence)?;
            device.record.device_name.clone_from(&request.device_name);
            device
                .record
                .agent_version
                .clone_from(&request.agent_version);
            device.record.platform = request.platform;
            device.record.capabilities = request.capabilities;
            return Ok(device.clone());
        }
        let device = StoredDevice {
            record: DeviceRecord {
                device_id: candidate_id.clone(),
                device_name: request.device_name.clone(),
                platform: request.platform,
                agent_version: request.agent_version.clone(),
                capabilities: request.capabilities,
                last_seen_ms: now_ms,
                online: true,
            },
            public_key,
            last_auth_nonce: 0,
        };
        self.public_keys
            .lock()
            .await
            .insert(public_key, candidate_id.clone());
        self.devices
            .lock()
            .await
            .insert(candidate_id, device.clone());
        Ok(device)
    }

    async fn get_device(&self, device_id: &DeviceId) -> Result<Option<StoredDevice>, ControlError> {
        Ok(self.devices.lock().await.get(device_id).cloned())
    }

    async fn heartbeat(
        &self,
        device_id: &DeviceId,
        request: &DeviceHeartbeatRequest,
        now_ms: u64,
    ) -> Result<(), ControlError> {
        let mut devices = self.devices.lock().await;
        let device = devices
            .get_mut(device_id)
            .ok_or(ControlError::DeviceNotFound)?;
        device.record.last_seen_ms = now_ms;
        device
            .record
            .agent_version
            .clone_from(&request.agent_version);
        device.record.platform = request.platform;
        device.record.capabilities = request.capabilities;
        Ok(())
    }

    async fn advance_nonce(&self, device_id: &DeviceId, nonce: u64) -> Result<bool, ControlError> {
        let mut devices = self.devices.lock().await;
        let device = devices
            .get_mut(device_id)
            .ok_or(ControlError::DeviceNotFound)?;
        if nonce <= device.last_auth_nonce {
            return Ok(false);
        }
        device.last_auth_nonce = nonce;
        Ok(true)
    }

    async fn create_session(&self, session: NewSession) -> Result<(), ControlError> {
        self.sessions.lock().await.push(MemorySession {
            session,
            authorization: MemoryAuthorization::Pending,
        });
        Ok(())
    }

    async fn pending_session(
        &self,
        device_id: &DeviceId,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError> {
        let sessions = self.sessions.lock().await;
        let Some(entry) = sessions.iter().find(|entry| {
            &entry.session.device_id == device_id
                && entry.authorization == MemoryAuthorization::Pending
                && entry.session.expires_at_ms >= now_ms
        }) else {
            return Ok(None);
        };
        Ok(Some(ClaimableSession {
            session_id: entry.session.session_id,
            controller_name: entry.session.controller_name.clone(),
            permissions: entry.session.permissions,
            agent_token_wrapped: entry.session.agent_token_wrapped.clone(),
            e2e_key_wrapped: entry.session.e2e_key_wrapped.clone(),
            expires_at_ms: entry.session.expires_at_ms,
            unattended_secret_wrapped: entry.session.unattended_secret_wrapped.clone(),
        }))
    }

    async fn resolve_authorization(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        granted_permissions: Option<SessionPermissions>,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError> {
        let mut sessions = self.sessions.lock().await;
        let Some(entry) = sessions.iter_mut().find(|entry| {
            entry.session.device_id == *device_id
                && entry.session.session_id == session_id
                && entry.authorization == MemoryAuthorization::Pending
                && entry.session.expires_at_ms >= now_ms
        }) else {
            return Ok(None);
        };
        entry.authorization = if granted_permissions.is_some() {
            MemoryAuthorization::Accepted
        } else {
            MemoryAuthorization::Rejected
        };
        Ok(Some(ClaimableSession {
            session_id,
            controller_name: entry.session.controller_name.clone(),
            permissions: granted_permissions.unwrap_or_default(),
            agent_token_wrapped: entry.session.agent_token_wrapped.clone(),
            e2e_key_wrapped: entry.session.e2e_key_wrapped.clone(),
            expires_at_ms: entry.session.expires_at_ms,
            unattended_secret_wrapped: entry.session.unattended_secret_wrapped.clone(),
        }))
    }

    async fn record_session_event(
        &self,
        device_id: &DeviceId,
        session_id: SessionId,
        event: &ReportSessionEventRequest,
        _now_ms: u64,
    ) -> Result<(), ControlError> {
        let mut sessions = self.sessions.lock().await;
        let entry = sessions
            .iter_mut()
            .find(|entry| {
                entry.session.device_id == *device_id && entry.session.session_id == session_id
            })
            .ok_or(ControlError::Conflict)?;
        match event.kind {
            SessionAuditEventKind::Started
                if entry.authorization == MemoryAuthorization::Accepted => {}
            SessionAuditEventKind::Ended
                if entry.authorization == MemoryAuthorization::Accepted =>
            {
                entry.authorization = MemoryAuthorization::Ended;
            }
            _ => return Err(ControlError::Conflict),
        }
        Ok(())
    }

    async fn record_audit(&self, event: AuditEvent) -> Result<(), ControlError> {
        self.audit_events.lock().await.push(event);
        Ok(())
    }
}

fn validate_registration(request: &DeviceRegistrationRequest) -> Result<(), ControlError> {
    if request.public_key.len() != 32
        || request.agent_version.is_empty()
        || request.agent_version.len() > 64
    {
        return Err(ControlError::InvalidRegistration);
    }
    validate_name(&request.device_name)
}

fn validate_name(name: &str) -> Result<(), ControlError> {
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(ControlError::InvalidName);
    }
    Ok(())
}

fn random_device_id() -> Result<DeviceId, ControlError> {
    let value = rand::rng().random_range(100_000_000_u32..=999_999_999);
    DeviceId::new(value.to_string()).map_err(|_| ControlError::InvalidDeviceId)
}

const fn audit_kind_text(kind: SessionAuditEventKind) -> &'static str {
    match kind {
        SessionAuditEventKind::Started => "started",
        SessionAuditEventKind::Ended => "ended",
    }
}

const fn connection_type_text(connection_type: ConnectionType) -> &'static str {
    match connection_type {
        ConnectionType::Relay => "relay",
        ConnectionType::Direct => "direct",
        ConnectionType::Lan => "lan",
    }
}

fn fixed_secret(value: Vec<u8>) -> Result<[u8; 32], ControlError> {
    value.try_into().map_err(|_| ControlError::Crypto)
}

const fn platform_text(platform: DevicePlatform) -> &'static str {
    match platform {
        DevicePlatform::Windows => "windows",
        DevicePlatform::Linux => "linux",
    }
}

fn parse_platform(value: &str) -> Result<DevicePlatform, ControlError> {
    match value {
        "windows" => Ok(DevicePlatform::Windows),
        "linux" => Ok(DevicePlatform::Linux),
        _ => Err(ControlError::Persistence),
    }
}

fn i64_value(value: u64) -> Result<i64, ControlError> {
    value.try_into().map_err(|_| ControlError::ClockOverflow)
}

fn u64_value(value: i64) -> Result<u64, ControlError> {
    value.try_into().map_err(|_| ControlError::Persistence)
}

fn now_ms() -> Result<u64, ControlError> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ControlError::ClockOverflow)?;
    duration
        .as_millis()
        .try_into()
        .map_err(|_| ControlError::ClockOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use remotex_crypto::{DeviceIdentity, Ed25519DeviceIdentity};

    fn config() -> ControlConfig {
        ControlConfig {
            relay_address: "relay.example.test:7443".into(),
            relay_server_name: "relay.example.test".into(),
            device_offline_after_ms: 100,
            session_lifetime_ms: 1_000,
        }
    }

    fn capabilities() -> SessionPermissions {
        SessionPermissions {
            view_desktop: true,
            control_input: true,
            clipboard: true,
            file_upload: true,
            file_download: true,
        }
    }

    async fn enrolled() -> (ControlService, Ed25519DeviceIdentity, DeviceId) {
        let (service, identity, device_id, _) = enrolled_with_repository().await;
        (service, identity, device_id)
    }

    async fn enrolled_with_repository() -> (
        ControlService,
        Ed25519DeviceIdentity,
        DeviceId,
        Arc<MemoryRepository>,
    ) {
        let repository = Arc::new(MemoryRepository::default());
        let service = ControlService::new(repository.clone(), [4; 32], config());
        let identity = Ed25519DeviceIdentity::from_secret_bytes([5; 32]);
        let response = service
            .register(
                DeviceRegistrationRequest {
                    public_key: identity.public_bytes().to_vec(),
                    device_name: "Test PC".into(),
                    platform: DevicePlatform::Windows,
                    agent_version: "0.1.0".into(),
                    capabilities: capabilities(),
                },
                1_000,
            )
            .await
            .expect("register device");
        (service, identity, response.device_id, repository)
    }

    fn proof(
        identity: &Ed25519DeviceIdentity,
        action: &str,
        device_id: &DeviceId,
        timestamp_ms: u64,
        nonce: u64,
    ) -> DeviceAuthProof {
        DeviceAuthProof {
            timestamp_ms,
            nonce,
            signature: identity
                .sign(&device_auth_message(
                    action,
                    device_id.as_str(),
                    timestamp_ms,
                    nonce,
                ))
                .expect("sign proof"),
        }
    }

    #[tokio::test]
    async fn registration_is_idempotent_for_one_public_key() {
        let (service, identity, device_id) = enrolled().await;
        let duplicate = service
            .register(
                DeviceRegistrationRequest {
                    public_key: identity.public_bytes().to_vec(),
                    device_name: "Renamed PC".into(),
                    platform: DevicePlatform::Windows,
                    agent_version: "0.1.1".into(),
                    capabilities: capabilities(),
                },
                1_100,
            )
            .await
            .expect("repeat registration");
        assert_eq!(duplicate.device_id, device_id);
    }

    #[tokio::test]
    async fn signed_heartbeat_updates_presence_and_rejects_replay() {
        let (service, identity, device_id) = enrolled().await;
        let request = DeviceHeartbeatRequest {
            proof: proof(&identity, "heartbeat", &device_id, 1_050, 1),
            agent_version: "0.1.0".into(),
            platform: DevicePlatform::Windows,
            capabilities: capabilities(),
        };
        service
            .heartbeat(&device_id, request.clone(), 1_050)
            .await
            .expect("heartbeat");
        assert!(
            service
                .device(&device_id, 1_100)
                .await
                .expect("device")
                .online
        );
        assert!(matches!(
            service.heartbeat(&device_id, request, 1_050).await,
            Err(ControlError::ReplayedProof)
        ));
        assert!(
            !service
                .device(&device_id, 1_200)
                .await
                .expect("device")
                .online
        );
    }

    #[tokio::test]
    async fn session_credentials_are_role_specific_and_agent_claims_once() {
        let (service, identity, device_id) = enrolled().await;
        let controller = service
            .create_session(
                CreateSessionRequest {
                    device_id: device_id.clone(),
                    controller_name: "Controller".into(),
                    requested_permissions: capabilities(),
                    unattended_secret: Some("correct horse".into()),
                },
                1_050,
            )
            .await
            .expect("create session");
        let incoming = service
            .claim_agent_session(
                &device_id,
                ClaimAgentSessionRequest {
                    proof: proof(&identity, "claim_session", &device_id, 1_060, 2),
                },
                1_060,
            )
            .await
            .expect("claim session")
            .authorization_request
            .expect("authorization request");
        assert_eq!(incoming.session_id, controller.session_id);
        assert_eq!(incoming.unattended_secret.as_deref(), Some("correct horse"));
        let action = format!("authorize_session:{}", controller.session_id);
        let agent = service
            .resolve_session_authorization(
                &device_id,
                controller.session_id,
                ResolveSessionAuthorizationRequest {
                    proof: proof(&identity, &action, &device_id, 1_070, 3),
                    decision: AuthorizationDecision::Accept,
                    granted_permissions: capabilities(),
                },
                1_070,
            )
            .await
            .expect("accept session")
            .credentials
            .expect("agent credentials");
        assert_eq!(agent.session_id, controller.session_id);
        assert_eq!(agent.end_to_end_key_hex, controller.end_to_end_key_hex);
        assert_ne!(agent.role_token_hex, controller.role_token_hex);
        let second = service
            .claim_agent_session(
                &device_id,
                ClaimAgentSessionRequest {
                    proof: proof(&identity, "claim_session", &device_id, 1_080, 4),
                },
                1_080,
            )
            .await
            .expect("second claim");
        assert!(second.authorization_request.is_none());
    }

    #[tokio::test]
    async fn offline_and_invalid_devices_cannot_create_sessions() {
        let (service, _, device_id) = enrolled().await;
        let request = CreateSessionRequest {
            device_id,
            controller_name: "Controller".into(),
            requested_permissions: capabilities(),
            unattended_secret: None,
        };
        assert!(matches!(
            service.create_session(request, 1_101).await,
            Err(ControlError::DeviceOffline)
        ));
        assert!(DeviceId::new("bad").is_err());
    }

    #[tokio::test]
    async fn expired_sessions_are_not_claimed() {
        let (service, identity, device_id) = enrolled().await;
        service
            .create_session(
                CreateSessionRequest {
                    device_id: device_id.clone(),
                    controller_name: "Controller".into(),
                    requested_permissions: capabilities(),
                    unattended_secret: None,
                },
                1_050,
            )
            .await
            .expect("create session");
        let claimed = service
            .claim_agent_session(
                &device_id,
                ClaimAgentSessionRequest {
                    proof: proof(&identity, "claim_session", &device_id, 2_051, 4),
                },
                2_051,
            )
            .await
            .expect("claim expired session");
        assert!(claimed.authorization_request.is_none());
    }

    #[tokio::test]
    async fn rejected_session_cannot_be_claimed_or_accepted_again() {
        let (service, identity, device_id) = enrolled().await;
        let controller = service
            .create_session(
                CreateSessionRequest {
                    device_id: device_id.clone(),
                    controller_name: "Rejected Controller".into(),
                    requested_permissions: capabilities(),
                    unattended_secret: None,
                },
                1_050,
            )
            .await
            .expect("create session");
        let action = format!("authorize_session:{}", controller.session_id);
        let rejected = service
            .resolve_session_authorization(
                &device_id,
                controller.session_id,
                ResolveSessionAuthorizationRequest {
                    proof: proof(&identity, &action, &device_id, 1_060, 2),
                    decision: AuthorizationDecision::Reject,
                    granted_permissions: capabilities(),
                },
                1_060,
            )
            .await
            .expect("reject session");
        assert!(rejected.credentials.is_none());
        assert!(matches!(
            service
                .resolve_session_authorization(
                    &device_id,
                    controller.session_id,
                    ResolveSessionAuthorizationRequest {
                        proof: proof(&identity, &action, &device_id, 1_070, 3),
                        decision: AuthorizationDecision::Accept,
                        granted_permissions: capabilities(),
                    },
                    1_070,
                )
                .await,
            Err(ControlError::Conflict)
        ));
    }

    #[tokio::test]
    async fn accepted_permissions_and_lifecycle_are_audited() {
        let (service, identity, device_id, repository) = enrolled_with_repository().await;
        let controller = service
            .create_session(
                CreateSessionRequest {
                    device_id: device_id.clone(),
                    controller_name: "Audited Controller".into(),
                    requested_permissions: capabilities(),
                    unattended_secret: None,
                },
                1_050,
            )
            .await
            .expect("create session");
        let granted = SessionPermissions {
            view_desktop: true,
            control_input: false,
            clipboard: true,
            file_upload: false,
            file_download: false,
        };
        let action = format!("authorize_session:{}", controller.session_id);
        let response = service
            .resolve_session_authorization(
                &device_id,
                controller.session_id,
                ResolveSessionAuthorizationRequest {
                    proof: proof(&identity, &action, &device_id, 1_060, 2),
                    decision: AuthorizationDecision::Accept,
                    granted_permissions: granted,
                },
                1_060,
            )
            .await
            .expect("accept session");
        assert_eq!(
            response.credentials.expect("credentials").permissions,
            granted
        );
        for (kind, timestamp, nonce, bytes, result) in [
            (SessionAuditEventKind::Started, 1_070, 3, 0, "active"),
            (
                SessionAuditEventKind::Ended,
                1_080,
                4,
                12_345,
                "disconnected",
            ),
        ] {
            let action = format!(
                "session_event:{}:{}",
                controller.session_id,
                audit_kind_text(kind)
            );
            service
                .report_session_event(
                    &device_id,
                    controller.session_id,
                    ReportSessionEventRequest {
                        proof: proof(&identity, &action, &device_id, timestamp, nonce),
                        kind,
                        connection_type: ConnectionType::Relay,
                        bytes_transferred: bytes,
                        result: result.into(),
                    },
                    timestamp,
                )
                .await
                .expect("record lifecycle event");
        }
        assert_eq!(repository.audit_event_count().await, 4);
    }
}
