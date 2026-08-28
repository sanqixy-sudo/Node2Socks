-- Rebuild subscriptions to relax refresh_interval_sec CHECK (manual refresh = 0)
-- and to widen download_mode CHECK with the 'system' proxy option.
-- migrate() disables foreign_keys around migrations, so the DROP below does not
-- cascade into nodes / refresh_runs; FK enforcement is restored afterwards.

CREATE TABLE subscriptions_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    url_cipher BLOB NOT NULL,
    refresh_interval_sec INTEGER NOT NULL DEFAULT 1800 CHECK (refresh_interval_sec >= 0),
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
    sync_version INTEGER NOT NULL DEFAULT 0 CHECK (sync_version >= 0),
    next_refresh_at TEXT,
    download_mode TEXT NOT NULL DEFAULT 'direct'
        CHECK (download_mode IN ('direct', 'system', 'custom_http', 'custom_socks5')),
    user_agent TEXT,
    proxy_url_cipher BLOB
);

INSERT INTO subscriptions_new (
    id, name, url_cipher, refresh_interval_sec, headers_cipher, enabled,
    content_format, etag, last_modified, last_success_at, last_error,
    cached_payload_cipher, created_at, updated_at, sync_version,
    next_refresh_at, download_mode, user_agent, proxy_url_cipher
)
SELECT
    id, name, url_cipher, refresh_interval_sec, headers_cipher, enabled,
    content_format, etag, last_modified, last_success_at, last_error,
    cached_payload_cipher, created_at, updated_at, sync_version,
    next_refresh_at, download_mode, user_agent, proxy_url_cipher
FROM subscriptions;

DROP TABLE subscriptions;
ALTER TABLE subscriptions_new RENAME TO subscriptions;

CREATE INDEX idx_subscriptions_next_refresh
    ON subscriptions(enabled, next_refresh_at);
