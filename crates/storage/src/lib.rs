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
        assert_eq!(schema_version(&connection).unwrap(), 4);
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
        assert_eq!(schema_version(&reopened).unwrap(), 4);
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
}
