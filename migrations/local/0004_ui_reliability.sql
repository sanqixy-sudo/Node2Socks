ALTER TABLE subscriptions ADD COLUMN proxy_url_cipher BLOB;
ALTER TABLE cloud_profiles ADD COLUMN refresh_token_cipher BLOB;

CREATE TABLE node_latency_results (
    node_id TEXT PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    delay_ms INTEGER,
    error_code TEXT,
    error_message TEXT,
    checked_at INTEGER NOT NULL,
    CHECK (delay_ms IS NOT NULL OR error_code IS NOT NULL)
);

CREATE INDEX idx_node_latency_checked_at
    ON node_latency_results(checked_at);

CREATE INDEX idx_slot_bindings_node_id
    ON slot_bindings(node_id);

PRAGMA optimize;
