CREATE TABLE sync_versions (
    record_type TEXT NOT NULL,
    record_id TEXT NOT NULL,
    cloud_version INTEGER NOT NULL DEFAULT 0 CHECK (cloud_version >= 0),
    PRIMARY KEY(record_type, record_id)
);
