-- Node2Socks Cloud v1 draft schema (SQLite)
-- The server stores account/device metadata plus opaque encrypted sync envelopes.
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,          -- Argon2id encoded hash
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    disabled_at INTEGER
);

CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    device_public_key TEXT,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_devices_user ON devices(user_id);

CREATE TABLE IF NOT EXISTS refresh_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    token_hash BLOB NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens(user_id);

-- Current encrypted state for every sync object.
-- The server does not need to understand payload_json; it only performs CAS/versioning.
CREATE TABLE IF NOT EXISTS sync_objects (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    object_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0,1)),
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    aad_version INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL,
    updated_by_device_id TEXT REFERENCES devices(id) ON DELETE SET NULL,
    PRIMARY KEY (user_id, object_type, object_id)
);
CREATE INDEX IF NOT EXISTS idx_sync_objects_user_updated ON sync_objects(user_id, updated_at);

-- Append-only change feed used for cursors/delta pull.
CREATE TABLE IF NOT EXISTS sync_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    object_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0,1)),
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    aad_version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    device_id TEXT REFERENCES devices(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_sync_events_user_seq ON sync_events(user_id, seq);

-- Optional encrypted vault bootstrap material, e.g. wrapped random vault key.
CREATE TABLE IF NOT EXISTS vault_bootstrap (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    kdf TEXT NOT NULL,
    kdf_params_json TEXT NOT NULL,
    salt BLOB NOT NULL,
    wrapped_vault_key BLOB NOT NULL,
    nonce BLOB NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL
);

-- Server housekeeping metadata.
CREATE TABLE IF NOT EXISTS server_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
