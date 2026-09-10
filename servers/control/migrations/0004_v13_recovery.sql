ALTER TABLE sessions
    ADD COLUMN controller_recovery_token_hash BYTEA,
    ADD COLUMN agent_recovery_token_hash BYTEA,
    ADD COLUMN agent_recovery_token_wrapped BYTEA,
    ADD COLUMN recovery_expires_at_ms BIGINT;

CREATE INDEX sessions_recovery_idx
    ON sessions(session_id, authorization_status, recovery_expires_at_ms);
