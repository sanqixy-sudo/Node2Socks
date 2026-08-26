PRAGMA foreign_keys = ON;

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    disabled_at INTEGER
);

CREATE TABLE devices (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    device_public_key TEXT,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX idx_devices_user ON devices(user_id);

CREATE TABLE refresh_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    token_hash BLOB NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX idx_refresh_tokens_user ON refresh_tokens(user_id);

CREATE TABLE sync_objects (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    object_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0,1)),
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    aad_version INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL,
    updated_by_device_id TEXT REFERENCES devices(id) ON DELETE SET NULL,
    PRIMARY KEY (user_id, object_type, object_id)
);
CREATE INDEX idx_sync_objects_user_updated ON sync_objects(user_id, updated_at);

CREATE TABLE sync_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    object_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    deleted INTEGER NOT NULL CHECK (deleted IN (0,1)),
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    aad_version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    device_id TEXT REFERENCES devices(id) ON DELETE SET NULL
);
CREATE INDEX idx_sync_events_user_seq ON sync_events(user_id, seq);

CREATE TABLE vault_bootstrap (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    kdf TEXT NOT NULL,
    kdf_params_json TEXT NOT NULL,
    salt BLOB NOT NULL,
    wrapped_vault_key BLOB NOT NULL,
    nonce BLOB NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL
);

CREATE TABLE server_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
