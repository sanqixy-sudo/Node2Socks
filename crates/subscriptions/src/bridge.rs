use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use rand::RngCore;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{RwLock, watch},
    task::JoinHandle,
};
use uuid::Uuid;

#[derive(Clone)]
struct BridgeState {
    token: String,
    payloads: Arc<RwLock<HashMap<Uuid, String>>>,
}

#[derive(Clone)]
pub struct ProviderBridge {
    state: BridgeState,
}

pub struct ProviderBridgeHandle {
    pub address: SocketAddr,
    pub token: String,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}
impl ProviderBridgeHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }
}

impl Default for ProviderBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderBridge {
    pub fn new() -> Self {
        let mut secret = [0_u8; 32];
        rand::rng().fill_bytes(&mut secret);
        Self {
            state: BridgeState {
                token: hex::encode(secret),
                payloads: Arc::new(RwLock::new(HashMap::new())),
            },
        }
    }

    pub async fn set_payload(&self, subscription_id: Uuid, payload: String) {
        self.state
            .payloads
            .write()
            .await
            .insert(subscription_id, payload);
    }

    pub async fn start(&self) -> AppResult<ProviderBridgeHandle> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(io_error)?;
        let address = listener.local_addr().map_err(io_error)?;
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let state = self.state.clone();
        let token = state.token.clone();
        let app = Router::new()
            .route("/provider/{subscription_id}", get(provider))
            .with_state(state);
        let task = tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    while !*shutdown_rx.borrow() {
                        if shutdown_rx.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await;
            if let Err(error) = result {
                tracing::error!(%error, "provider bridge stopped unexpectedly");
            }
        });
        Ok(ProviderBridgeHandle {
            address,
            token,
            shutdown,
            task,
        })
    }
}

async fn provider(
    State(state): State<BridgeState>,
    Path(subscription_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {}", state.token));
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match state.payloads.read().await.get(&subscription_id).cloned() {
        Some(payload) => (
            [(header::CONTENT_TYPE, "text/yaml; charset=utf-8")],
            payload,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::new(ErrorCode::IoError, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bridge_is_localhost_and_requires_token() {
        let bridge = ProviderBridge::new();
        let id = Uuid::new_v4();
        bridge.set_payload(id, "proxies: []\n".into()).await;
        let handle = bridge.start().await.unwrap();
        assert!(handle.address.ip().is_loopback());
        let url = format!("http://{}/provider/{id}", handle.address);
        assert_eq!(reqwest::get(&url).await.unwrap().status(), 401);
        let response = reqwest::Client::new()
            .get(url)
            .bearer_auth(&handle.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "proxies: []\n");
        handle.shutdown().await;
    }
}
