use crate::{backend::ProductState, cloud_commands};
use node2socks_core_adapter::{
    ProxyCore,
    mihomo::{MihomoConfig, MihomoManager},
    provider::ProviderSource,
    topology::{CoreSlot, CoreTopology, slot_selector_name},
};
use node2socks_diagnostics::NetworkAdapter;
use node2socks_domain::{ProxySlot, SlotBinding, SlotBindingState};
use node2socks_runtime_service::{HealthChecker, HealthResult, SlotReconciler};
use node2socks_slot_manager::{
    PortAllocator, PortRange, SlotRepository, SqliteSlotRepository, SystemPortProbe,
};
use node2socks_subscriptions::{
    CatalogNode, DownloadMode, SubscriptionRecord, SubscriptionRepository, SubscriptionService,
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, SystemTime},
};
use tauri::State;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionView {
    pub id: String,
    pub name: String,
    pub masked_url: String,
    pub node_count: usize,
    pub enabled: bool,
    pub last_status: String,
    pub last_success_at: Option<u64>,
    pub last_error: Option<String>,
    pub next_refresh_at: Option<u64>,
    pub manual_refresh: bool,
    pub has_error: bool,
}

pub(crate) fn subscription_views(state: &ProductState) -> Result<Vec<SubscriptionView>, String> {
    let repository = subscription_repository(state)?;
    let nodes = repository.nodes().map_err(text)?;
    repository
        .list()
        .map_err(text)?
        .into_iter()
        .map(|item| {
            let count = nodes
                .iter()
                .filter(|node| node.subscription_id == item.id && node.present)
                .count();
            let has_error = item.last_error.is_some();
            Ok(SubscriptionView {
                id: item.id.to_string(),
                name: item.name,
                masked_url: mask_url(&item.url),
                node_count: count,
                enabled: item.enabled,
                last_status: item.last_error.clone().unwrap_or_else(|| {
                    item.last_success_at
                        .map(|v| format!("{v} 更新"))
                        .unwrap_or_else(|| "尚未刷新".into())
                }),
                last_success_at: item.last_success_at,
                last_error: item.last_error.clone(),
                next_refresh_at: item.next_refresh_at,
                manual_refresh: item.refresh_interval_sec == 0,
                has_error,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn create_subscription(
    state: State<'_, ProductState>,
    name: String,
    url: String,
) -> Result<String, String> {
    let item = SubscriptionRecord {
        id: Uuid::new_v4(),
        name: name.trim().to_owned(),
        url: url.trim().to_owned(),
        enabled: true,
        refresh_interval_sec: 1800,
        next_refresh_at: None,
        last_success_at: None,
        last_error: None,
        download_mode: DownloadMode::Direct,
        user_agent: None,
        headers: Vec::new(),
        proxy_url: None,
        revision: 0,
    };
    if item.name.is_empty() {
        return Err("订阅名称不能为空".into());
    }
    url::Url::parse(&item.url)
        .map_err(|e| e.to_string())
        .and_then(|value| {
            matches!(value.scheme(), "http" | "https")
                .then_some(())
                .ok_or_else(|| "订阅仅支持 HTTP/HTTPS".into())
        })?;
    subscription_repository(&state)?
        .upsert(&item)
        .map_err(text)?;
    if state.core_running.load(Ordering::Relaxed) {
        rebuild_core(&state).await?;
    }
    cloud_commands::enqueue_subscription(&state, item.id).await?;
    Ok(item.id.to_string())
}
#[tauri::command]
pub async fn delete_subscription(state: State<'_, ProductState>, id: String) -> Result<(), String> {
    let id = parse_id(&id)?;
    let node_ids: HashSet<_> = subscription_repository(&state)?
        .nodes()
        .map_err(text)?
        .into_iter()
        .filter(|node| node.subscription_id == id)
        .map(|node| node.id)
        .collect();
    let bound_ports: Vec<_> = slot_repository(&state)?
        .list()
        .map_err(text)?
        .into_iter()
        .filter(|(_, binding)| {
            binding
                .node_id
                .is_some_and(|node_id| node_ids.contains(&node_id))
        })
        .map(|(slot, _)| slot.local_port)
        .collect();
    if !bound_ports.is_empty() {
        return Err(format!(
            "订阅仍被固定端口 {} 使用。请先把这些 Slot 绑定到其他节点或删除 Slot；为防止出口意外变化，本次删除已取消。",
            bound_ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    subscription_repository(&state)?.delete(id).map_err(text)?;
    cloud_commands::enqueue_tombstone(&state, "subscription", id).await?;
    if state.core_running.load(Ordering::Relaxed) {
        rebuild_core(&state).await?;
    }
    Ok(())
}
#[tauri::command]
pub async fn refresh_subscription(
    state: State<'_, ProductState>,
    id: String,
) -> Result<usize, String> {
    let id = parse_id(&id)?;
    refresh_subscription_inner(&state, id, &CancellationToken::new()).await
}

pub(crate) async fn refresh_subscription_inner(
    state: &ProductState,
    id: Uuid,
    cancel: &CancellationToken,
) -> Result<usize, String> {
    let service =
        SubscriptionService::new(subscription_repository(state)?, state.bridge.clone(), 4)
            .map_err(text)?;
    let result = service.refresh(id, cancel).await.map_err(text)?;
    if !result.diff.disappeared.is_empty() {
        if let Some(manager) = state.core.lock().await.as_ref() {
            let controller = manager.controller().await.map_err(text)?;
            let repository = slot_repository(state)?;
            let reconciler = SlotReconciler::new(repository, controller);
            reconciler
                .fail_closed_disappeared(&result.diff.disappeared.into_iter().collect())
                .await
                .map_err(text)?;
        } else {
            let disappeared: HashSet<_> = result.diff.disappeared.iter().copied().collect();
            let repository = slot_repository(state)?;
            for (slot, binding) in repository.list().map_err(text)? {
                if binding
                    .node_id
                    .is_some_and(|node_id| disappeared.contains(&node_id))
                {
                    repository
                        .bind(slot.id, binding.node_id, SlotBindingState::Orphaned)
                        .map_err(text)?;
                }
            }
        }
    }
    cloud_commands::enqueue_subscription(state, id).await?;
    if state.core_running.load(Ordering::Relaxed) {
        rebuild_core(state).await?;
    }
    Ok(result.node_count)
}
#[tauri::command]
pub fn list_nodes(state: State<'_, ProductState>) -> Result<Vec<CatalogNode>, String> {
    subscription_repository(&state)?.nodes().map_err(text)
}

#[tauri::command]
pub async fn create_slot(state: State<'_, ProductState>, name: String) -> Result<String, String> {
    let repository = slot_repository(&state)?;
    let settings = crate::advanced_commands::load_settings(&state)?;
    let allocator = PortAllocator::new(
        PortRange {
            start: settings.port_start,
            end: settings.port_end,
        },
        Duration::from_secs(settings.cooldown_hours * 60 * 60),
        SystemPortProbe,
    )
    .map_err(text)?;
    let port = allocator
        .allocate(
            &repository.used_ports().map_err(text)?,
            &repository.cooldowns().map_err(text)?,
            SystemTime::now(),
        )
        .map_err(text)?;
    let slot = ProxySlot::new(
        if name.trim().is_empty() {
            format!("Slot {port}")
        } else {
            name.trim().to_owned()
        },
        port,
    )
    .map_err(text)?;
    repository
        .create(
            &slot,
            &SlotBinding {
                slot_id: slot.id,
                node_id: None,
                state: SlotBindingState::Unbound,
                revision: 0,
            },
        )
        .map_err(text)?;
    cloud_commands::enqueue_slot(&state, slot.id).await?;
    if state.core_running.load(Ordering::Relaxed) {
        rebuild_core(&state).await?;
    }
    Ok(slot.id.to_string())
}
#[tauri::command]
pub async fn delete_slot(state: State<'_, ProductState>, id: String) -> Result<(), String> {
    let settings = crate::advanced_commands::load_settings(&state)?;
    slot_repository(&state)?
        .delete_with_cooldown(
            parse_id(&id)?,
            Duration::from_secs(settings.cooldown_hours * 60 * 60),
        )
        .map_err(text)?;
    if state.core_running.load(Ordering::Relaxed) {
        rebuild_core(&state).await?;
    }
    cloud_commands::enqueue_tombstone(&state, "slot", parse_id(&id)?).await?;
    Ok(())
}
#[tauri::command]
pub async fn bind_slot(
    state: State<'_, ProductState>,
    slot_id: String,
    node_id: String,
) -> Result<(), String> {
    let slot_id = parse_id(&slot_id)?;
    let node_id = parse_id(&node_id)?;
    let node = subscription_repository(&state)?
        .nodes()
        .map_err(text)?
        .into_iter()
        .find(|node| node.id == node_id && node.present)
        .ok_or_else(|| "节点不存在或已从订阅消失".to_owned())?;
    if let Some(manager) = state.core.lock().await.as_ref() {
        let controller = manager.controller().await.map_err(text)?;
        let selector = slot_selector_name(slot_id);
        controller
            .select(&selector, &node.internal_name)
            .await
            .map_err(text)?;
        if controller.selected(&selector).await.map_err(text)? != node.internal_name {
            return Err("Core 未确认新的 Slot 绑定".into());
        }
    }
    slot_repository(&state)?
        .bind(slot_id, Some(node_id), SlotBindingState::Active)
        .map_err(text)?;
    cloud_commands::enqueue_slot(&state, slot_id).await
}

#[tauri::command]
pub async fn check_slot(
    state: State<'_, ProductState>,
    id: String,
) -> Result<HealthResult, String> {
    if !state.core_running.load(Ordering::Relaxed) {
        return Err("请先启动 Core".into());
    }
    let id = parse_id(&id)?;
    let (slot, binding) = slot_repository(&state)?
        .list()
        .map_err(text)?
        .into_iter()
        .find(|(slot, _)| slot.id == id)
        .ok_or_else(|| "Slot 不存在".to_owned())?;
    if binding.state != SlotBindingState::Active {
        return Err("只有已绑定且未阻断的 Slot 可以执行链路检测".into());
    }
    let health = HealthChecker::new("https://api.ipify.org?format=json", Duration::from_secs(12))
        .check_socks(slot.local_port, &CancellationToken::new())
        .await
        .map_err(text)?;
    state.health_results.lock().await.insert(id, health.clone());
    Ok(health)
}

#[tauri::command]
pub async fn start_core(state: State<'_, ProductState>) -> Result<u32, String> {
    start_core_inner(&state).await
}

pub(crate) async fn start_core_inner(state: &ProductState) -> Result<u32, String> {
    if let Some(manager) = state.core.lock().await.as_ref() {
        return manager
            .health()
            .await
            .map_err(text)?
            .pid
            .ok_or_else(|| "Core PID 不可用".into());
    }
    let bridge_info = {
        let handle = state.bridge_handle.lock().await;
        let handle = handle
            .as_ref()
            .ok_or_else(|| "Provider Bridge 未运行".to_owned())?;
        (handle.address, handle.token.clone())
    };
    let repository = subscription_repository(state)?;
    let nodes = repository.nodes().map_err(text)?;
    let node_by_id: HashMap<_, _> = nodes.iter().map(|node| (node.id, node)).collect();
    let slots = slot_repository(state)?
        .list()
        .map_err(text)?
        .into_iter()
        .map(|(slot, binding)| CoreSlot {
            id: slot.id,
            local_port: slot.local_port,
            selected: binding
                .node_id
                .and_then(|id| node_by_id.get(&id))
                .filter(|node| node.present)
                .map(|node| node.internal_name.clone()),
        })
        .collect();
    let topology = CoreTopology {
        slots,
        available_nodes: nodes
            .iter()
            .filter(|node| node.present)
            .map(|node| node.internal_name.clone())
            .collect(),
    };
    for slot in &topology.slots {
        if let Err(conflict) = PortAllocator::new(
            PortRange {
                start: 1,
                end: u16::MAX,
            },
            Duration::ZERO,
            SystemPortProbe,
        )
        .map_err(text)?
        .validate_stable_port(slot.local_port)
        {
            return Err(format!(
                "端口 {} 已被占用{}{}；Node2Socks 不会自动修改已保存端口",
                conflict.port,
                conflict
                    .process_name
                    .as_ref()
                    .map(|name| format!("，进程 {name}"))
                    .unwrap_or_default(),
                conflict
                    .pid
                    .map(|pid| format!("，PID {pid}"))
                    .unwrap_or_default()
            ));
        }
    }
    let subscriptions = repository.list().map_err(text)?;
    let mut providers = Vec::new();
    for item in subscriptions.into_iter().filter(|item| item.enabled) {
        // A newly added or failed subscription has no normalized payload yet. Excluding it
        // keeps Mihomo healthy and avoids a noisy localhost 404; a successful refresh
        // rebuilds the Core and adds the provider immediately.
        if repository.cached_payload(item.id).map_err(text)?.is_none() {
            continue;
        }
        providers.push(ProviderSource {
            subscription_id: item.id,
            url: format!("http://{}/provider/{}", bridge_info.0, item.id),
            bearer_token: bridge_info.1.clone(),
            interval_seconds: item.refresh_interval_sec,
        });
    }
    let executable = resolve_sidecar()?;
    let runtime = database_path(state)?
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("runtime");
    let mut config = MihomoConfig::new(executable, runtime);
    config.topology = Some(topology);
    config.providers = providers;
    config.outbound_interface = crate::advanced_commands::desired_outbound_interface(state)?;
    let manager = Arc::new(MihomoManager::new(config).map_err(text)?);
    let health = manager.start().await.map_err(text)?;
    let pid = health.pid.ok_or_else(|| "Core PID 不可用".to_owned())?;
    let monitor = manager.clone().spawn_crash_monitor(Duration::from_secs(1));
    *state.core.lock().await = Some(manager);
    *state.crash_monitor.lock().await = Some(monitor);
    state.core_running.store(true, Ordering::Relaxed);
    Ok(pid)
}
#[tauri::command]
pub async fn stop_core(state: State<'_, ProductState>) -> Result<(), String> {
    stop_core_inner(&state).await
}

pub(crate) async fn stop_core_inner(state: &ProductState) -> Result<(), String> {
    if let Some(monitor) = state.crash_monitor.lock().await.take() {
        monitor.shutdown().await;
    }
    if let Some(manager) = state.core.lock().await.take() {
        manager.stop().await.map_err(text)?;
    }
    state.core_running.store(false, Ordering::Relaxed);
    Ok(())
}

pub(crate) async fn rebuild_core(state: &ProductState) -> Result<(), String> {
    stop_core_inner(state).await?;
    start_core_inner(state).await?;
    Ok(())
}
pub(crate) fn subscription_repository(
    state: &ProductState,
) -> Result<SubscriptionRepository, String> {
    let key = state
        .master_key
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "主密钥未初始化".to_owned())?;
    Ok(SubscriptionRepository::new(
        node2socks_storage::open_and_migrate(database_path(state)?).map_err(text)?,
        key,
    ))
}
pub(crate) fn slot_repository(state: &ProductState) -> Result<SqliteSlotRepository, String> {
    Ok(SqliteSlotRepository::new(
        node2socks_storage::open_and_migrate(database_path(state)?).map_err(text)?,
    ))
}
pub(crate) fn database_path(state: &ProductState) -> Result<PathBuf, String> {
    state
        .database
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "数据库未初始化".into())
}
fn resolve_sidecar() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or_else(|| "无法定位程序目录".to_owned())?;
    let candidates = [
        parent.join("node2socks-mihomo.exe"),
        parent.join("node2socks-mihomo-x86_64-pc-windows-msvc.exe"),
        PathBuf::from("sidecar/windows-x64/node2socks-mihomo.exe"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "找不到 node2socks-mihomo.exe".into())
}

#[tauri::command]
pub fn list_network_adapters() -> Result<Vec<NetworkAdapter>, String> {
    node2socks_diagnostics::inspect_windows()
        .map(|report| report.physical_adapters)
        .map_err(text)
}

#[tauri::command]
pub async fn set_outbound_interface(
    state: State<'_, ProductState>,
    interface_name: Option<String>,
) -> Result<(), String> {
    let value = interface_name
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(name) = &value {
        let valid = node2socks_diagnostics::inspect_windows()
            .map_err(text)?
            .physical_adapters
            .into_iter()
            .any(|adapter| adapter.up && adapter.name == *name);
        if !valid {
            return Err("指定网卡不存在或当前未连接".into());
        }
    }
    let connection = node2socks_storage::open_and_migrate(database_path(&state)?).map_err(text)?;
    connection.execute(
        "INSERT INTO app_settings(key,value_json,scope,updated_at,sync_version) VALUES('outbound_interface',?1,'device_local',strftime('%s','now'),0) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,updated_at=excluded.updated_at,sync_version=sync_version+1",
        [serde_json::to_string(&value).map_err(text)?],
    ).map_err(text)?;
    if state.core_running.load(Ordering::Relaxed) {
        rebuild_core(&state).await?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_outbound_interface(state: State<'_, ProductState>) -> Result<Option<String>, String> {
    outbound_interface(&state)
}

fn outbound_interface(state: &ProductState) -> Result<Option<String>, String> {
    let connection = node2socks_storage::open_and_migrate(database_path(state)?).map_err(text)?;
    let value: Option<String> = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key='outbound_interface'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(text)?;
    value
        .map(|raw| serde_json::from_str(&raw).map_err(text))
        .transpose()
        .map(|value| value.flatten())
}
fn parse_id(value: &str) -> Result<Uuid, String> {
    Uuid::parse_str(value).map_err(|e| e.to_string())
}
fn mask_url(value: &str) -> String {
    match url::Url::parse(value) {
        Ok(mut url) => {
            if url.query().is_some() {
                url.set_query(Some("token=••••••"));
            }
            url.to_string()
        }
        Err(_) => "[INVALID URL]".into(),
    }
}
fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}
