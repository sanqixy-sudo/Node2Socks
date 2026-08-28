//! Versioned local SQLite migrations and connection policy.

use node2socks_domain::{AppError, AppResult, ErrorCode};
use rusqlite::{Connection, TransactionBehavior};
use std::path::Path;

const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        include_str!("../../../migrations/local/0001_initial.sql"),
    ),
    (
        2,
        include_str!("../../../migrations/local/0002_subscription_runtime.sql"),
    ),
    (
        3,
        include_str!("../../../migrations/local/0003_sync_versions.sql"),
    ),
    (
        4,
        include_str!("../../../migrations/local/0004_ui_reliability.sql"),
    ),
    (
        5,
        include_str!("../../../migrations/local/0005_subscription_modes.sql"),
    ),
    (
        6,
        include_str!("../../../migrations/local/0006_download_via_node.sql"),
    ),
];

pub fn open_and_migrate(path: impl AsRef<Path>) -> AppResult<Connection> {
    let mut connection = Connection::open(path).map_err(database_error)?;
    configure(&connection)?;
    migrate(&mut connection)?;
    Ok(connection)
}

pub fn open_in_memory_and_migrate() -> AppResult<Connection> {
    let mut connection = Connection::open_in_memory().map_err(database_error)?;
    configure(&connection)?;
    migrate(&mut connection)?;
    Ok(connection)
}

fn configure(connection: &Connection) -> AppResult<()> {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;\n\
             PRAGMA busy_timeout = 5000;\n\
             PRAGMA journal_mode = WAL;",
        )
        .map_err(database_error)
}

pub fn migrate(connection: &mut Connection) -> AppResult<()> {
    let current: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(database_error)?;

    if !MIGRATIONS.iter().any(|(version, _)| *version > current) {
        return Ok(());
    }

    // Table-rebuild migrations (e.g. 0005) drop and recreate tables; with
    // foreign key enforcement active, DROP TABLE implicitly deletes all rows
    // and cascades into child tables. SQLite only honors the pragma outside a
    // transaction, so enforcement is toggled around the whole migration run.
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .map_err(database_error)?;
    let result = apply_migrations(connection, current);
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(database_error)?;
    result?;

    let mut check = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(database_error)?;
    let violations = check
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .count();
    if violations > 0 {
        return Err(AppError::new(
            ErrorCode::DatabaseError,
            format!("foreign key check failed after migration: {violations} violation(s)"),
        ));
    }
    Ok(())
}

fn apply_migrations(connection: &mut Connection, current: u32) -> AppResult<()> {
    for (version, sql) in MIGRATIONS {
        if *version <= current {
            continue;
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        transaction.execute_batch(sql).map_err(database_error)?;
        transaction
            .pragma_update(None, "user_version", version)
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
    }
    Ok(())
}

pub fn schema_version(connection: &Connection) -> AppResult<u32> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(database_error)
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::new(ErrorCode::DatabaseError, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fresh_database_migrates_to_latest_version() {
        let connection = open_in_memory_and_migrate().unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 6);
        let count: u32 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='proxy_slots'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let sync_versions_pk_columns: u32 = connection
            .query_row(
                "SELECT count(*) FROM pragma_table_info('sync_versions') WHERE pk > 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sync_versions_pk_columns, 2);
        connection.execute("INSERT INTO sync_versions(record_type,record_id,cloud_version) VALUES('slot','id-1',4) ON CONFLICT(record_type,record_id) DO UPDATE SET cloud_version=excluded.cloud_version", []).unwrap();
        let version: u64 = connection.query_row("SELECT cloud_version FROM sync_versions WHERE record_type='slot' AND record_id='id-1'", [], |row| row.get(0)).unwrap();
        assert_eq!(version, 4);
    }

    #[test]
    fn migration_is_idempotent_after_reopen() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("app.db");
        drop(open_and_migrate(&database).unwrap());
        let reopened = open_and_migrate(&database).unwrap();
        assert_eq!(schema_version(&reopened).unwrap(), 6);
    }

    #[test]
    fn slot_listener_cannot_be_non_localhost() {
        let connection = open_in_memory_and_migrate().unwrap();
        let result = connection.execute(
            "INSERT INTO proxy_slots (id,name,local_port,listen_host,created_at,updated_at) VALUES ('s','x',21001,'0.0.0.0','now','now')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn subscriptions_accept_manual_refresh_and_system_proxy_mode() {
        let connection = open_in_memory_and_migrate().unwrap();
        connection
            .execute(
                "INSERT INTO subscriptions (id,name,url_cipher,refresh_interval_sec,download_mode,created_at,updated_at) VALUES ('s','x',X'00',0,'system','now','now')",
                [],
            )
            .unwrap();
        let (interval, mode): (u64, String) = connection
            .query_row(
                "SELECT refresh_interval_sec, download_mode FROM subscriptions WHERE id='s'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(interval, 0);
        assert_eq!(mode, "system");
        let bad_interval = connection.execute(
            "INSERT INTO subscriptions (id,name,url_cipher,refresh_interval_sec,created_at,updated_at) VALUES ('b','x',X'00',-1,'now','now')",
            [],
        );
        assert!(bad_interval.is_err());
        let bad_mode = connection.execute(
            "INSERT INTO subscriptions (id,name,url_cipher,download_mode,created_at,updated_at) VALUES ('c','x',X'00','bogus','now','now')",
            [],
        );
        assert!(bad_mode.is_err());
    }

    #[test]
    fn subscriptions_accept_node_download_mode_and_download_node_id() {
        let connection = open_in_memory_and_migrate().unwrap();
        connection
            .execute(
                "INSERT INTO subscriptions (id,name,url_cipher,download_mode,download_node_id,created_at,updated_at) VALUES ('s','x',X'00','node','00000000-0000-0000-0000-00000000000a','now','now')",
                [],
            )
            .unwrap();
        let (mode, node_id): (String, Option<String>) = connection
            .query_row(
                "SELECT download_mode, download_node_id FROM subscriptions WHERE id='s'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(mode, "node");
        assert_eq!(
            node_id.as_deref(),
            Some("00000000-0000-0000-0000-00000000000a")
        );
        // Rows written before the 0006 rebuild default to no download node.
        connection
            .execute(
                "INSERT INTO subscriptions (id,name,url_cipher,download_mode,created_at,updated_at) VALUES ('d','x',X'00','direct','now','now')",
                [],
            )
            .unwrap();
        let legacy: Option<String> = connection
            .query_row(
                "SELECT download_node_id FROM subscriptions WHERE id='d'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, None);
    }

    #[test]
    fn rebuild_migration_preserves_existing_rows_and_children() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("app.db");
        let connection = open_and_migrate(&database).unwrap();
        // Rows present before the 0005 table rebuild must survive, together
        // with their child rows (nodes references subscriptions).
        connection.execute("INSERT INTO subscriptions (id,name,url_cipher,created_at,updated_at) VALUES ('s','x',X'00','now','now')", []).unwrap();
        connection.execute("INSERT INTO nodes (id,subscription_id,stable_key,internal_name,upstream_name,provider_name,created_at,updated_at) VALUES ('n','s','k','i','u','p','now','now')", []).unwrap();
        drop(connection);
        let reopened = open_and_migrate(&database).unwrap();
        let nodes: u32 = reopened
            .query_row(
                "SELECT count(*) FROM nodes WHERE subscription_id='s'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(nodes, 1);
    }
}
