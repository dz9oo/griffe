CREATE TABLE audit_log (
    id             TEXT PRIMARY KEY,
    sequence       INTEGER NOT NULL UNIQUE,
    occurred_at    TEXT NOT NULL,
    actor_kind     TEXT NOT NULL,
    actor_session  TEXT,
    command_name   TEXT NOT NULL,
    command_json   TEXT NOT NULL,
    outcome        TEXT NOT NULL,
    output_json    TEXT,
    previous_hash  TEXT,
    hash           TEXT NOT NULL
);
CREATE INDEX idx_audit_log_sequence ON audit_log(sequence);

CREATE TABLE pending_actions (
    id             TEXT PRIMARY KEY,
    actor_session  TEXT NOT NULL,
    command_name   TEXT NOT NULL,
    command_json   TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    status         TEXT NOT NULL,
    resolved_at    TEXT
);
CREATE INDEX idx_pending_actions_status ON pending_actions(status);

CREATE TABLE idempotency_keys (
    key           TEXT PRIMARY KEY,
    command_name  TEXT NOT NULL,
    output_json   TEXT NOT NULL,
    recorded_at   TEXT NOT NULL
);
