ALTER TABLE subscriptions ADD COLUMN next_refresh_at TEXT;
ALTER TABLE subscriptions ADD COLUMN download_mode TEXT NOT NULL DEFAULT 'direct'
    CHECK (download_mode IN ('direct', 'custom_http', 'custom_socks5'));
ALTER TABLE subscriptions ADD COLUMN user_agent TEXT;

CREATE INDEX idx_subscriptions_next_refresh
    ON subscriptions(enabled, next_refresh_at);

CREATE TABLE refresh_runs (
    id TEXT PRIMARY KEY,
    subscription_id TEXT NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    outcome TEXT CHECK (outcome IN ('success', 'failed', 'cancelled')),
    error_code TEXT
);
