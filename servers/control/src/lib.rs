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
    ClaimAgentSessionRequest, ClaimAgentSessionResponse, CreateSessionRequest, DeviceAuthProof,
    DeviceHeartbeatRequest, DeviceId, DevicePlatform, DeviceRecord, DeviceRegistrationRequest,
    DeviceRegistrationResponse, SessionCredentials, SessionId, SessionPermissions,
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
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct ClaimableSession {
    pub session_id: SessionId,
    pub permissions: SessionPermissions,
    pub agent_token_wrapped: Vec<u8>,
    pub e2e_key_wrapped: Vec<u8>,
    pub expires_at_ms: u64,
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
    async fn claim_agent_session(
        &self,
        device_id: &DeviceId,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError>;
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
        let device = self
            .repository
            .get_device(&request.device_id)
            .await?
            .ok_or(ControlError::DeviceNotFound)?;
        if now_ms.saturating_sub(device.record.last_seen_ms) > self.config.device_offline_after_ms {
            return Err(ControlError::DeviceOffline);
        }
        let permissions =
            intersect_permissions(request.requested_permissions, device.record.capabilities);
        let controller_token: [u8; 32] = rand::random();
        let agent_token: [u8; 32] = rand::random();
        let end_to_end_key: [u8; 32] = rand::random();
        let session_id = SessionId::new();
        let expires_at_ms = now_ms
            .checked_add(self.config.session_lifetime_ms)
            .ok_or(ControlError::ClockOverflow)?;
        self.repository
            .create_session(NewSession {
                session_id,
                device_id: request.device_id,
                controller_name: request.controller_name,
                permissions,
                controller_token_hash: secret_hash(&controller_token),
                agent_token_hash: secret_hash(&agent_token),
                agent_token_wrapped: self.secrets.seal(&agent_token)?,
                e2e_key_wrapped: self.secrets.seal(&end_to_end_key)?,
                created_at_ms: now_ms,
                expires_at_ms,
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
        let Some(session) = self
            .repository
            .claim_agent_session(device_id, now_ms)
            .await?
        else {
            return Ok(ClaimAgentSessionResponse { credentials: None });
        };
        let agent_token = fixed_secret(self.secrets.open(&session.agent_token_wrapped)?)?;
        let e2e_key = fixed_secret(self.secrets.open(&session.e2e_key_wrapped)?)?;
        Ok(ClaimAgentSessionResponse {
            credentials: Some(self.credentials(
                session.session_id,
                agent_token,
                e2e_key,
                session.expires_at_ms,
                session.permissions,
            )),
        })
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
            Self::InvalidDeviceId | Self::InvalidRegistration | Self::InvalidName => {
                (StatusCode::BAD_REQUEST, "invalid_request")
            }
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
    #[error("device registration is invalid")]
    InvalidRegistration,
    #[error("name is invalid")]
    InvalidName,
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
            Self::InvalidRegistration => "device registration is invalid",
            Self::InvalidName => "name is invalid",
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
        sqlx::query("INSERT INTO sessions (session_id, device_id, controller_name, permissions_json, controller_token_hash, agent_token_hash, agent_token_wrapped, e2e_key_wrapped, created_at_ms, expires_at_ms) VALUES ($1::uuid,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
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
            .execute(&self.pool)
            .await
            .map_err(|_| ControlError::Persistence)?;
        Ok(())
    }

    async fn claim_agent_session(
        &self,
        device_id: &DeviceId,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| ControlError::Persistence)?;
        let row = sqlx::query("SELECT session_id::text, permissions_json, agent_token_wrapped, e2e_key_wrapped, expires_at_ms FROM sessions WHERE device_id=$1 AND agent_claimed=FALSE AND expires_at_ms >= $2 ORDER BY created_at_ms LIMIT 1 FOR UPDATE SKIP LOCKED")
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
        let session_id_text: String = row.try_get(0).map_err(|_| ControlError::Persistence)?;
        sqlx::query("UPDATE sessions SET agent_claimed=TRUE WHERE session_id=$1::uuid")
            .bind(&session_id_text)
            .execute(&mut *transaction)
            .await
            .map_err(|_| ControlError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| ControlError::Persistence)?;
        Ok(Some(ClaimableSession {
            session_id: session_id_text
                .parse()
                .map_err(|_| ControlError::Persistence)?,
            permissions: serde_json::from_str(
                row.try_get::<&str, _>(1)
                    .map_err(|_| ControlError::Persistence)?,
            )
            .map_err(|_| ControlError::Persistence)?,
            agent_token_wrapped: row.try_get(2).map_err(|_| ControlError::Persistence)?,
            e2e_key_wrapped: row.try_get(3).map_err(|_| ControlError::Persistence)?,
            expires_at_ms: u64_value(row.try_get(4).map_err(|_| ControlError::Persistence)?)?,
        }))
    }
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

#[derive(Default)]
pub struct MemoryRepository {
    devices: Mutex<HashMap<DeviceId, StoredDevice>>,
    public_keys: Mutex<HashMap<[u8; 32], DeviceId>>,
    sessions: Mutex<Vec<(NewSession, bool)>>,
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
        self.sessions.lock().await.push((session, false));
        Ok(())
    }

    async fn claim_agent_session(
        &self,
        device_id: &DeviceId,
        now_ms: u64,
    ) -> Result<Option<ClaimableSession>, ControlError> {
        let mut sessions = self.sessions.lock().await;
        let Some((session, claimed)) = sessions.iter_mut().find(|(session, claimed)| {
            &session.device_id == device_id && !*claimed && session.expires_at_ms >= now_ms
        }) else {
            return Ok(None);
        };
        *claimed = true;
        Ok(Some(ClaimableSession {
            session_id: session.session_id,
            permissions: session.permissions,
            agent_token_wrapped: session.agent_token_wrapped.clone(),
            e2e_key_wrapped: session.e2e_key_wrapped.clone(),
            expires_at_ms: session.expires_at_ms,
        }))
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

const fn intersect_permissions(
    requested: SessionPermissions,
    available: SessionPermissions,
) -> SessionPermissions {
    SessionPermissions {
        view_desktop: requested.view_desktop && available.view_desktop,
        control_input: requested.control_input && available.control_input,
        clipboard: requested.clipboard && available.clipboard,
        file_upload: requested.file_upload && available.file_upload,
        file_download: requested.file_download && available.file_download,
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
        let service = ControlService::new(Arc::new(MemoryRepository::default()), [4; 32], config());
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
        (service, identity, response.device_id)
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
                },
                1_050,
            )
            .await
            .expect("create session");
        let agent = service
            .claim_agent_session(
                &device_id,
                ClaimAgentSessionRequest {
                    proof: proof(&identity, "claim_session", &device_id, 1_060, 2),
                },
                1_060,
            )
            .await
            .expect("claim session")
            .credentials
            .expect("agent credentials");
        assert_eq!(agent.session_id, controller.session_id);
        assert_eq!(agent.end_to_end_key_hex, controller.end_to_end_key_hex);
        assert_ne!(agent.role_token_hex, controller.role_token_hex);
        let second = service
            .claim_agent_session(
                &device_id,
                ClaimAgentSessionRequest {
                    proof: proof(&identity, "claim_session", &device_id, 1_070, 3),
                },
                1_070,
            )
            .await
            .expect("second claim");
        assert!(second.credentials.is_none());
    }

    #[tokio::test]
    async fn offline_and_invalid_devices_cannot_create_sessions() {
        let (service, _, device_id) = enrolled().await;
        let request = CreateSessionRequest {
            device_id,
            controller_name: "Controller".into(),
            requested_permissions: capabilities(),
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
        assert!(claimed.credentials.is_none());
    }
}
