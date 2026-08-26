PRAGMA foreign_keys = ON;

CREATE TABLE subscriptions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    url_cipher BLOB NOT NULL,
    refresh_interval_sec INTEGER NOT NULL DEFAULT 1800 CHECK (refresh_interval_sec > 0),
    headers_cipher BLOB,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    content_format TEXT,
    etag TEXT,
    last_modified TEXT,
    last_success_at TEXT,
    last_error TEXT,
    cached_payload_cipher BLOB,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    sync_version INTEGER NOT NULL DEFAULT 0 CHECK (sync_version >= 0)
);

CREATE TABLE nodes (
    id TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    stable_key TEXT NOT NULL,
    internal_name TEXT NOT NULL,
    upstream_name TEXT NOT NULL,
    protocol TEXT,
    provider_name TEXT NOT NULL,
    last_seen_at TEXT,
    is_present INTEGER NOT NULL DEFAULT 1 CHECK (is_present IN (0, 1)),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(subscription_id, stable_key)
);

CREATE INDEX idx_nodes_subscription ON nodes(subscription_id);

CREATE TABLE proxy_slots (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    local_port INTEGER NOT NULL UNIQUE CHECK (local_port BETWEEN 1 AND 65535),
    listen_host TEXT NOT NULL DEFAULT '127.0.0.1' CHECK (listen_host = '127.0.0.1'),
    username_cipher BLOB,
    password_cipher BLOB,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    sync_version INTEGER NOT NULL DEFAULT 0 CHECK (sync_version >= 0)
);

CREATE TABLE slot_bindings (
    slot_id TEXT PRIMARY KEY REFERENCES proxy_slots(id) ON DELETE CASCADE,
    node_id TEXT REFERENCES nodes(id) ON DELETE SET NULL,
    state TEXT NOT NULL DEFAULT 'unbound'
        CHECK (state IN ('active', 'orphaned', 'unbound', 'blocked', 'error')),
    last_applied_internal_name TEXT,
    updated_at TEXT NOT NULL,
    sync_version INTEGER NOT NULL DEFAULT 0 CHECK (sync_version >= 0)
);

CREATE TABLE app_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('synced', 'device_local')),
    updated_at TEXT NOT NULL,
    sync_version INTEGER NOT NULL DEFAULT 0 CHECK (sync_version >= 0)
);

CREATE TABLE cloud_profiles (
    id TEXT PRIMARY KEY,
    base_url TEXT NOT NULL,
    account_name TEXT NOT NULL,
    device_id TEXT NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 0 CHECK (is_active IN (0, 1)),
    last_cursor INTEGER NOT NULL DEFAULT 0 CHECK (last_cursor >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_cloud_profile_active
    ON cloud_profiles(is_active) WHERE is_active = 1;

CREATE TABLE sync_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    record_type TEXT NOT NULL,
    record_id TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    base_version INTEGER NOT NULL CHECK (base_version >= 0),
    payload_cipher BLOB NOT NULL,
    created_at TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT
);

CREATE TABLE port_cooldowns (
    local_port INTEGER PRIMARY KEY CHECK (local_port BETWEEN 1 AND 65535),
    released_at TEXT NOT NULL,
    reusable_after TEXT NOT NULL
);
