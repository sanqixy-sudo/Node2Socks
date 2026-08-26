use axum::{Json, Router, routing::get};
pub mod api;
use api::{CloudState, routes};

use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use std::path::Path;

pub const API_VERSION: &str = "1";
const MIGRATIONS: &[(u32, &str)] = &[(
    1,
    include_str!("../../../migrations/cloud/0001_initial.sql"),
)];

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub service: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub service: &'static str,
    pub version: &'static str,
    pub api_version: &'static str,
    pub registration_enabled: bool,
    pub max_payload_bytes: usize,
}

pub fn router() -> Router {
    let mut database = Connection::open_in_memory().expect("in-memory cloud database");
    migrate(&mut database).expect("cloud migration");
    let state = CloudState::new(
        database,
        b"node2socks-development-secret-change-me".to_vec(),
    );
    router_with_state(state)
}

pub fn router_with_state(state: CloudState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/server-info", get(server_info))
        .merge(routes(state))
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "node2socks-cloud",
    })
}

async fn server_info() -> Json<ServerInfo> {
    Json(ServerInfo {
        service: "node2socks-cloud",
        version: env!("CARGO_PKG_VERSION"),
        api_version: API_VERSION,
        registration_enabled: true,
        max_payload_bytes: 5 * 1024 * 1024,
    })
}

pub fn open_and_migrate(path: impl AsRef<Path>) -> rusqlite::Result<Connection> {
    let mut connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;\nPRAGMA busy_timeout = 5000;\nPRAGMA journal_mode = WAL;",
    )?;
    migrate(&mut connection)?;
    Ok(connection)
}

pub fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let current: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (version, sql) in MIGRATIONS {
        if *version <= current {
            continue;
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(sql)?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_endpoint_is_live() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    #[test]
    fn fresh_cloud_database_migrates() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }
}
