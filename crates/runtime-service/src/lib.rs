//! Application service that keeps persisted Slot truth synchronized with the proxy core.

use async_trait::async_trait;
use node2socks_core_adapter::{controller::MihomoController, topology::slot_selector_name};
use node2socks_domain::{AppError, AppResult, ErrorCode, SlotBindingState};
use node2socks_slot_manager::SlotRepository;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    net::IpAddr,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[async_trait]
pub trait SelectorControl: Send + Sync {
    async fn select(&self, selector: &str, target: &str) -> AppResult<()>;
    async fn selected(&self, selector: &str) -> AppResult<String>;
}

#[async_trait]
impl SelectorControl for MihomoController {
    async fn select(&self, selector: &str, target: &str) -> AppResult<()> {
        MihomoController::select(self, selector, target).await
    }
    async fn selected(&self, selector: &str) -> AppResult<String> {
        MihomoController::selected(self, selector).await
    }
}

pub struct SlotReconciler<R, C> {
    repository: R,
    controller: C,
}

impl<R: SlotRepository, C: SelectorControl> SlotReconciler<R, C> {
    pub fn new(repository: R, controller: C) -> Self {
        Self {
            repository,
            controller,
        }
    }

    /// BLOCK is applied to Mihomo before the database advertises the orphaned state.
    pub async fn fail_closed_disappeared(
        &self,
        disappeared: &HashSet<Uuid>,
    ) -> AppResult<Vec<Uuid>> {
        let mut orphaned = Vec::new();
        for (slot, binding) in self.repository.list()? {
            if let Some(node_id) = binding.node_id
                && disappeared.contains(&node_id)
            {
                let selector = slot_selector_name(slot.id);
                self.controller.select(&selector, "REJECT").await?;
                if self.controller.selected(&selector).await? != "REJECT" {
                    return Err(AppError::new(
                        ErrorCode::CoreUnhealthy,
                        "selector did not confirm REJECT",
                    ));
                }
                self.repository
                    .bind(slot.id, Some(node_id), SlotBindingState::Orphaned)?;
                orphaned.push(slot.id);
            }
        }
        Ok(orphaned)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthResult {
    pub latency_ms: u64,
    pub exit_ip: String,
    pub country: Option<String>,
}

#[derive(Clone)]
pub struct HealthChecker {
    endpoints: Vec<String>,
    timeout: Duration,
}

impl HealthChecker {
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Self {
        Self {
            endpoints: vec![endpoint.into()],
            timeout,
        }
    }

    pub fn public_ip(timeout: Duration) -> Self {
        Self {
            endpoints: [
                "https://api.ipify.org?format=json",
                "https://api64.ipify.org?format=json",
                "https://httpbin.org/ip",
                "https://www.cloudflare.com/cdn-cgi/trace",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            timeout,
        }
    }

    pub async fn check_socks(
        &self,
        port: u16,
        cancel: &CancellationToken,
    ) -> AppResult<HealthResult> {
        let proxy =
            reqwest::Proxy::all(format!("socks5h://127.0.0.1:{port}")).map_err(config_error)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(proxy)
            .connect_timeout(self.timeout)
            .timeout(self.timeout)
            .build()
            .map_err(config_error)?;
        let started = Instant::now();
        let mut failures = Vec::new();
        for endpoint in &self.endpoints {
            let request = client.get(endpoint).send();
            let response = tokio::select! {
                response = request => match response {
                    Ok(response) => response,
                    Err(error) => {
                        failures.push(format!("{}: {}", endpoint_host(endpoint), error));
                        continue;
                    }
                },
                _ = cancel.cancelled() => return Err(AppError::new(ErrorCode::OperationCancelled, "health check cancelled")),
            };
            if !response.status().is_success() {
                failures.push(format!(
                    "{}: HTTP {}",
                    endpoint_host(endpoint),
                    response.status()
                ));
                continue;
            }
            let body = match response.text().await {
                Ok(body) => body,
                Err(error) => {
                    failures.push(format!("{}: {}", endpoint_host(endpoint), error));
                    continue;
                }
            };
            if let Some((exit_ip, country)) = parse_health_body(&body) {
                return Ok(HealthResult {
                    latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    exit_ip,
                    country,
                });
            }
            failures.push(format!("{}: 返回未包含有效 IP", endpoint_host(endpoint)));
        }
        Err(AppError::new(
            ErrorCode::NodeUnavailable,
            format!("出口检测失败：{}", failures.join("; ")),
        ))
    }
}

fn endpoint_host(endpoint: &str) -> &str {
    endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
        .unwrap_or(endpoint)
        .split('/')
        .next()
        .unwrap_or(endpoint)
}

fn parse_health_body(body: &str) -> Option<(String, Option<String>)> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        let raw_ip = value
            .get("ip")
            .or_else(|| value.get("origin"))
            .and_then(|value| value.as_str())?;
        let ip = parse_ip(raw_ip)?;
        let country = value
            .get("country_code")
            .or_else(|| value.get("country"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_owned());
        return Some((ip, country));
    }
    let trace: std::collections::HashMap<_, _> = body
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    if let Some(ip) = trace.get("ip").and_then(|value| parse_ip(value)) {
        let country = trace
            .get("loc")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        return Some((ip, country));
    }
    parse_ip(body).map(|ip| (ip, None))
}

fn parse_ip(raw: &str) -> Option<String> {
    let candidate = raw.split(',').next()?.trim();
    candidate.parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

fn config_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidConfiguration, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use node2socks_domain::{ProxySlot, SlotBinding};
    use node2socks_slot_manager::{SlotRepository, SqliteSlotRepository};
    use node2socks_storage::open_in_memory_and_migrate;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct FakeControl {
        selected: Mutex<HashMap<String, String>>,
        events: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl SelectorControl for Arc<FakeControl> {
        async fn select(&self, selector: &str, target: &str) -> AppResult<()> {
            self.events.lock().unwrap().push(format!("core:{target}"));
            self.selected
                .lock()
                .unwrap()
                .insert(selector.into(), target.into());
            Ok(())
        }
        async fn selected(&self, selector: &str) -> AppResult<String> {
            Ok(self
                .selected
                .lock()
                .unwrap()
                .get(selector)
                .cloned()
                .unwrap_or_default())
        }
    }

    #[tokio::test]
    async fn disappeared_bound_node_is_confirmed_reject_and_binding_is_preserved() {
        let connection = open_in_memory_and_migrate().unwrap();
        connection.execute("INSERT INTO subscriptions(id,name,url_cipher,created_at,updated_at) VALUES('00000000-0000-0000-0000-000000000001','s',x'00','0','0')",[]).unwrap();
        let node = Uuid::new_v4();
        connection.execute("INSERT INTO nodes(id,subscription_id,stable_key,internal_name,upstream_name,provider_name,created_at,updated_at) VALUES(?1,'00000000-0000-0000-0000-000000000001','k','[s] n','n','p','0','0')",[node.to_string()]).unwrap();
        let repository = SqliteSlotRepository::new(connection);
        let slot = ProxySlot::new("slot", 21001).unwrap();
        repository
            .create(
                &slot,
                &SlotBinding {
                    slot_id: slot.id,
                    node_id: Some(node),
                    state: SlotBindingState::Active,
                    revision: 0,
                },
            )
            .unwrap();
        let control = Arc::new(FakeControl::default());
        let reconciler = SlotReconciler::new(repository.clone(), control.clone());
        assert_eq!(
            reconciler
                .fail_closed_disappeared(&HashSet::from([node]))
                .await
                .unwrap(),
            vec![slot.id]
        );
        let (_, binding) = repository.list().unwrap().remove(0);
        assert_eq!(binding.node_id, Some(node));
        assert_eq!(binding.state, SlotBindingState::Orphaned);
        assert_eq!(control.events.lock().unwrap().as_slice(), ["core:REJECT"]);
    }

    #[tokio::test]
    async fn cancelled_health_check_stops_without_waiting() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = HealthChecker::new("https://example.invalid", Duration::from_secs(30))
            .check_socks(9, &cancel)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::OperationCancelled);
    }
}
