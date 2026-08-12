CREATE TABLE IF NOT EXISTS devices (
    device_id CHAR(9) PRIMARY KEY,
    public_key BYTEA NOT NULL UNIQUE,
    device_name VARCHAR(128) NOT NULL,
    platform VARCHAR(16) NOT NULL,
    agent_version VARCHAR(64) NOT NULL,
    capabilities_json TEXT NOT NULL,
    registered_at_ms BIGINT NOT NULL,
    last_seen_ms BIGINT NOT NULL,
    last_auth_nonce BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id UUID PRIMARY KEY,
    device_id CHAR(9) NOT NULL REFERENCES devices(device_id),
    controller_name VARCHAR(128) NOT NULL,
    permissions_json TEXT NOT NULL,
    controller_token_hash BYTEA NOT NULL,
    agent_token_hash BYTEA NOT NULL,
    agent_token_wrapped BYTEA NOT NULL,
    e2e_key_wrapped BYTEA NOT NULL,
    controller_consumed BOOLEAN NOT NULL DEFAULT FALSE,
    agent_consumed BOOLEAN NOT NULL DEFAULT FALSE,
    agent_claimed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms BIGINT NOT NULL,
    expires_at_ms BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS sessions_device_claim_idx
    ON sessions(device_id, agent_claimed, expires_at_ms, created_at_ms);

CREATE TABLE IF NOT EXISTS audit_events (
    audit_id BIGSERIAL PRIMARY KEY,
    session_id UUID NULL REFERENCES sessions(session_id),
    device_id CHAR(9) NULL REFERENCES devices(device_id),
    event_type VARCHAR(64) NOT NULL,
    occurred_at_ms BIGINT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);
