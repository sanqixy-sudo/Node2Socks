-- Draft domain schema. Codex must convert this into real versioned migrations.
CREATE TABLE subscriptions (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  url_cipher BLOB NOT NULL,
  refresh_interval_sec INTEGER NOT NULL DEFAULT 1800,
  headers_cipher BLOB,
  enabled INTEGER NOT NULL DEFAULT 1,
  content_format TEXT,
  etag TEXT,
  last_modified TEXT,
  last_success_at TEXT,
  last_error TEXT,
  cached_payload_cipher BLOB,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  sync_version INTEGER NOT NULL DEFAULT 0
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
  is_present INTEGER NOT NULL DEFAULT 1,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(subscription_id, stable_key)
);

CREATE TABLE proxy_slots (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  local_port INTEGER NOT NULL UNIQUE,
  listen_host TEXT NOT NULL DEFAULT '127.0.0.1',
  username_cipher BLOB,
  password_cipher BLOB,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  sync_version INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE slot_bindings (
  slot_id TEXT PRIMARY KEY REFERENCES proxy_slots(id) ON DELETE CASCADE,
  node_id TEXT REFERENCES nodes(id) ON DELETE SET NULL,
  state TEXT NOT NULL DEFAULT 'unbound',
  last_applied_internal_name TEXT,
  updated_at TEXT NOT NULL,
  sync_version INTEGER NOT NULL DEFAULT 0
);
