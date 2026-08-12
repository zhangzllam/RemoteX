ALTER TABLE sessions
    ADD COLUMN authorization_status VARCHAR(16) NOT NULL DEFAULT 'pending',
    ADD COLUMN authorized_permissions_json TEXT NULL,
    ADD COLUMN unattended_secret_wrapped BYTEA NULL,
    ADD COLUMN authorized_at_ms BIGINT NULL,
    ADD COLUMN started_at_ms BIGINT NULL,
    ADD COLUMN ended_at_ms BIGINT NULL,
    ADD COLUMN connection_type VARCHAR(16) NULL,
    ADD COLUMN bytes_transferred BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN result VARCHAR(64) NULL;

ALTER TABLE sessions
    ADD CONSTRAINT sessions_authorization_status_check
    CHECK (authorization_status IN ('pending', 'accepted', 'rejected', 'ended'));

CREATE INDEX sessions_pending_authorization_idx
    ON sessions(device_id, authorization_status, expires_at_ms, created_at_ms);
