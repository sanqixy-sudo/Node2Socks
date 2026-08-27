use crate::{
    backend::{NodeLatencyProbe, ProductState},
    cloud_commands, commands,
};
use node2socks_domain::{DEFAULT_PORT_END, DEFAULT_PORT_START, ProxySlot, SlotBindingState};
use node2socks_runtime_service::{HealthChecker, HealthResult};
use node2socks_slot_manager::{PortAllocator, PortRange, SlotRepository, SystemPortProbe};
use node2socks_subscriptions::{DownloadMode, SubscriptionRecord};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::Ordering,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionDetail {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub refresh_interval_sec: u64,
    pub next_refresh_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub last_error: Option<String>,
    pub download_mode: String,
    pub user_agent: Option<String>,
    pub proxy_url: Option<String>,
    pub headers: Vec<HeaderEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionInput {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub refresh_interval_sec: u64,
    pub download_mode: String,
    pub user_agent: Option<String>,
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub headers: Vec<HeaderEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub theme: String,
    pub density: String,
    pub sidebar_collapsed: bool,
    pub port_start: u16,
    pub port_end: u16,
    pub cooldown_hours: u64,
    pub start_in_tray: bool,
    pub auto_start_core: bool,
    pub outbound_mode: String,
    pub outbound_interface: Option<String>,
    pub node_group_expansion: BTreeMap<String, bool>,
}
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            density: "compact".into(),
            sidebar_collapsed: false,
            port_start: DEFAULT_PORT_START,
            port_end: DEFAULT_PORT_END,
            cooldown_hours: 24,
            start_in_tray: false,
            auto_start_core: true,
            outbound_mode: "system".into(),
            outbound_interface: None,
            node_group_expansion: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundSlotView {
    pub id: String,
    pub port: u16,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub id: String,
    pub subscription_id: String,
    pub subscription_name: String,
    pub display_name: String,
    pub protocol: String,
    pub present: bool,
    pub bound_slots: Vec<BoundSlotView>,
    pub latency_ms: Option<u64>,
    pub latency_error: Option<String>,
    pub latency_checked_at: Option<u64>,
    pub exit_ip: Option<String>,
    pub country: Option<String>,
}

#[tauri::command]
pub fn get_settings(state: State<'_, ProductState>) -> Result<AppSettings, String> {
    load_settings(&state)
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, ProductState>,
    settings: AppSettings,
) -> Result<MutationResult<AppSettings>, String> {
    validate_settings(&settings)?;
    let before = load_settings(&state)?;
    let values = [
        ("theme", serde_json::json!(settings.theme), "synced"),
        ("density", serde_json::json!(settings.density), "synced"),
        (
            "sidebar_collapsed",
            serde_json::json!(settings.sidebar_collapsed),
            "device_local",
        ),
        (
            "port_start",
            serde_json::json!(settings.port_start),
            "synced",
        ),
        ("port_end", serde_json::json!(settings.port_end), "synced"),
        (
            "cooldown_hours",
            serde_json::json!(settings.cooldown_hours),
            "synced",
        ),
        (
            "start_in_tray",
            serde_json::json!(settings.start_in_tray),
            "device_local",
        ),
        (
            "auto_start_core",
            serde_json::json!(settings.auto_start_core),
            "device_local",
        ),
        (
            "outbound_mode",
            serde_json::json!(settings.outbound_mode),
            "device_local",
        ),
        (
            "outbound_interface",
            serde_json::json!(settings.outbound_interface),
            "device_local",
        ),
        (
            "node_group_expansion",
            serde_json::json!(settings.node_group_expansion),
            "device_local",
        ),
    ];
    {
        let mut connection =
            node2socks_storage::open_and_migrate(commands::database_path(&state)?).map_err(text)?;
        let transaction = connection.transaction().map_err(text)?;
        for (key, value, scope) in values {
            transaction.execute("INSERT INTO app_settings(key,value_json,scope,updated_at,sync_version) VALUES(?1,?2,?3,strftime('%s','now'),0) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,scope=excluded.scope,updated_at=excluded.updated_at,sync_version=app_settings.sync_version+1",params![key,value.to_string(),scope]).map_err(text)?;
        }
        transaction.commit().map_err(text)?;
    }
    let mut warnings = Vec::new();
    if let Err(error) = cloud_commands::enqueue_settings(&state).await {
        warnings.push(format!("云同步队列写入失败：{error}"));
    }
    if state.core_running.load(Ordering::Relaxed)
        && (before.outbound_mode != settings.outbound_mode
            || before.outbound_interface != settings.outbound_interface)
    {
        if let Err(error) = commands::rebuild_core(&state).await {
            warnings.push(format!("设置已保存，但 Core 应用失败：{error}"));
        }
    }
    Ok(MutationResult {
        value: settings,
        warning: (!warnings.is_empty()).then(|| warnings.join("；")),
    })
}

pub(crate) fn load_settings(state: &ProductState) -> Result<AppSettings, String> {
    let connection =
        node2socks_storage::open_and_migrate(commands::database_path(state)?).map_err(text)?;
    let mut result = AppSettings::default();
    let mut statement = connection
        .prepare("SELECT key,value_json FROM app_settings")
        .map_err(text)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(text)?;
    for row in rows {
        let (key, raw) = row.map_err(text)?;
        match key.as_str() {
            "theme" => result.theme = decode(&raw)?,
            "density" => result.density = decode(&raw)?,
            "sidebar_collapsed" => result.sidebar_collapsed = decode(&raw)?,
            "port_start" => result.port_start = decode(&raw)?,
            "port_end" => result.port_end = decode(&raw)?,
            "cooldown_hours" => result.cooldown_hours = decode(&raw)?,
            "start_in_tray" => result.start_in_tray = decode(&raw)?,
            "auto_start_core" => result.auto_start_core = decode(&raw)?,
            "outbound_mode" => result.outbound_mode = decode(&raw)?,
            "outbound_interface" => result.outbound_interface = decode(&raw)?,
            "node_group_expansion" => result.node_group_expansion = decode(&raw)?,
            _ => {}
        }
    }
    validate_settings(&result)?;
    Ok(result)
}

fn validate_settings(value: &AppSettings) -> Result<(), String> {
    if !matches!(value.theme.as_str(), "system" | "light" | "dark") {
        return Err("主题设置无效".into());
    }
    if !matches!(value.density.as_str(), "compact" | "comfortable") {
        return Err("界面密度设置无效".into());
    }
    if value.port_start < 1024 || value.port_start > value.port_end {
        return Err("端口范围必须位于 1024–65535 且起始端口不能大于结束端口".into());
    }
    if !(1..=720).contains(&value.cooldown_hours) {
        return Err("端口冷却必须在 1–720 小时之间".into());
    }
    if !matches!(value.outbound_mode.as_str(), "system" | "auto" | "manual") {
        return Err("出站模式无效".into());
    }
    if value.outbound_mode == "manual" {
        let name = value
            .outbound_interface
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .ok_or("手动出站模式必须选择网卡")?;
        let valid = node2socks_diagnostics::inspect_windows()
            .map_err(text)?
            .physical_adapters
            .into_iter()
            .any(|a| a.up && a.name == name);
        if !valid {
            return Err("指定网卡不存在或当前未连接".into());
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_subscription(
    state: State<'_, ProductState>,
    id: String,
) -> Result<SubscriptionDetail, String> {
    let item = commands::subscription_repository(&state)?
        .get(parse_id(&id)?)
        .map_err(text)?
        .ok_or("订阅不存在")?;
    Ok(detail(item))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationResult<T> {
    pub value: T,
    pub warning: Option<String>,
}

#[tauri::command]
pub async fn save_subscription(
    state: State<'_, ProductState>,
    id: Option<String>,
    input: SubscriptionInput,
) -> Result<MutationResult<String>, String> {
    validate_subscription(&input)?;
    let repository = commands::subscription_repository(&state)?;
    let parsed_id = id.as_deref().map(parse_id).transpose()?;
    let previous = parsed_id
        .map(|id| repository.get(id))
        .transpose()
        .map_err(text)?
        .flatten();
    if parsed_id.is_some() && previous.is_none() {
        return Err("订阅不存在".into());
    }
    let record_id = parsed_id.unwrap_or_else(Uuid::new_v4);
    let item = SubscriptionRecord {
        id: record_id,
        name: input.name.trim().into(),
        url: input.url.trim().into(),
        enabled: input.enabled,
        refresh_interval_sec: input.refresh_interval_sec,
        next_refresh_at: if input.refresh_interval_sec == 0 {
            None
        } else {
            previous.as_ref().and_then(|item| item.next_refresh_at)
        },
        last_success_at: previous.as_ref().and_then(|item| item.last_success_at),
        last_error: previous.as_ref().and_then(|item| item.last_error.clone()),
        download_mode: mode(&input.download_mode)?,
        user_agent: clean(input.user_agent),
        headers: input
            .headers
            .into_iter()
            .map(|header| (header.name.trim().to_owned(), header.value))
            .filter(|(name, _)| !name.is_empty())
            .collect(),
        proxy_url: clean(input.proxy_url),
        revision: previous
            .as_ref()
            .map_or(0, |item| item.revision.saturating_add(1)),
    };
    repository.upsert(&item).map_err(text)?;
    let mut warnings = Vec::new();
    if let Err(error) = cloud_commands::enqueue_subscription(&state, record_id).await {
        warnings.push(format!("云同步队列写入失败：{error}"));
    }
    if state.core_running.load(Ordering::Relaxed) {
        if let Err(error) = commands::rebuild_core(&state).await {
            warnings.push(format!("数据已保存，但 Core 应用失败：{error}"));
        }
    }
    Ok(MutationResult {
        value: record_id.to_string(),
        warning: (!warnings.is_empty()).then(|| warnings.join("；")),
    })
}

#[tauri::command]
pub async fn update_subscription(
    state: State<'_, ProductState>,
    id: String,
    input: SubscriptionInput,
) -> Result<(), String> {
    validate_subscription(&input)?;
    let id = parse_id(&id)?;
    let repository = commands::subscription_repository(&state)?;
    let previous = repository.get(id).map_err(text)?.ok_or("订阅不存在")?;
    let item = SubscriptionRecord {
        id,
        name: input.name.trim().into(),
        url: input.url.trim().into(),
        enabled: input.enabled,
        refresh_interval_sec: input.refresh_interval_sec,
        next_refresh_at: previous.next_refresh_at,
        last_success_at: previous.last_success_at,
        last_error: previous.last_error,
        download_mode: mode(&input.download_mode)?,
        user_agent: clean(input.user_agent),
        proxy_url: clean(input.proxy_url),
        headers: input
            .headers
            .into_iter()
            .map(|h| (h.name.trim().to_owned(), h.value))
            .filter(|(k, _)| !k.is_empty())
            .collect(),
        revision: previous.revision.saturating_add(1),
    };
    repository.upsert(&item).map_err(text)?;
    cloud_commands::enqueue_subscription(&state, id).await?;
    if state.core_running.load(Ordering::Relaxed) {
        commands::rebuild_core(&state).await?
    }
    Ok(())
}

#[tauri::command]
pub async fn set_subscription_enabled(
    state: State<'_, ProductState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let id = parse_id(&id)?;
    let repository = commands::subscription_repository(&state)?;
    let mut item = repository.get(id).map_err(text)?.ok_or("订阅不存在")?;
    item.enabled = enabled;
    item.revision = item.revision.saturating_add(1);
    repository.upsert(&item).map_err(text)?;
    cloud_commands::enqueue_subscription(&state, id).await?;
    if state.core_running.load(Ordering::Relaxed) {
        commands::rebuild_core(&state).await?
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSummary {
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: Vec<RefreshFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshFailure {
    pub id: String,
    pub name: String,
    pub error: String,
}

#[tauri::command]
pub async fn refresh_all_subscriptions(
    state: State<'_, ProductState>,
) -> Result<RefreshSummary, String> {
    let subscriptions = commands::subscription_repository(&state)?
        .list()
        .map_err(text)?;
    let skipped = subscriptions.iter().filter(|item| !item.enabled).count();
    let mut summary = RefreshSummary {
        succeeded: 0,
        failed: 0,
        skipped,
        errors: Vec::new(),
    };
    for item in subscriptions.into_iter().filter(|item| item.enabled) {
        match commands::refresh_subscription_inner(&state, item.id, &CancellationToken::new()).await
        {
            Ok(_) => summary.succeeded += 1,
            Err(_) => {
                summary.failed += 1;
                summary.errors.push(RefreshFailure {
                    id: item.id.to_string(),
                    name: item.name,
                    error: "刷新失败，请打开订阅详情查看原因".into(),
                });
            }
        }
    }
    Ok(summary)
}

#[tauri::command]
pub async fn batch_create_slots(
    state: State<'_, ProductState>,
    node_ids: Vec<String>,
    name_prefix: Option<String>,
) -> Result<MutationResult<Vec<String>>, String> {
    if node_ids.is_empty() {
        return Err("请至少选择一个节点".into());
    }
    let ids: Vec<Uuid> = node_ids
        .iter()
        .map(|id| parse_id(id))
        .collect::<Result<_, _>>()?;
    let nodes = commands::subscription_repository(&state)?
        .nodes()
        .map_err(text)?;
    if ids
        .iter()
        .any(|id| !nodes.iter().any(|n| n.id == *id && n.present))
    {
        return Err("所选节点中存在已消失节点".into());
    }
    let settings = load_settings(&state)?;
    let repository = commands::slot_repository(&state)?;
    let mut used = repository.used_ports().map_err(text)?;
    let cooldowns = repository.cooldowns().map_err(text)?;
    let allocator = PortAllocator::new(
        PortRange {
            start: settings.port_start,
            end: settings.port_end,
        },
        Duration::from_secs(settings.cooldown_hours * 3600),
        SystemPortProbe,
    )
    .map_err(text)?;
    let mut pending = Vec::new();
    for (index, node_id) in ids.into_iter().enumerate() {
        let port = allocator
            .allocate(&used, &cooldowns, SystemTime::now())
            .map_err(text)?;
        used.insert(port);
        let name = name_prefix
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|p| format!("{} {:02}", p.trim(), index + 1))
            .unwrap_or_else(|| {
                nodes
                    .iter()
                    .find(|n| n.id == node_id)
                    .map(|n| n.display_name.clone())
                    .unwrap_or_else(|| format!("Slot {port}"))
            });
        pending.push((ProxySlot::new(name, port).map_err(text)?, node_id));
    }
    {
        let mut connection =
            node2socks_storage::open_and_migrate(commands::database_path(&state)?).map_err(text)?;
        let tx = connection.transaction().map_err(text)?;
        let now = now()?.to_string();
        for (slot, node_id) in &pending {
            tx.execute("INSERT INTO proxy_slots(id,name,local_port,listen_host,enabled,created_at,updated_at,sync_version) VALUES(?1,?2,?3,'127.0.0.1',1,?4,?4,0)",params![slot.id.to_string(),slot.name,slot.local_port,now]).map_err(text)?;
            tx.execute("INSERT INTO slot_bindings(slot_id,node_id,state,updated_at,sync_version) VALUES(?1,?2,'active',?3,0)",params![slot.id.to_string(),node_id.to_string(),now]).map_err(text)?;
        }
        tx.commit().map_err(text)?;
    }
    let mut warnings = Vec::new();
    for (slot, _) in &pending {
        if let Err(error) = cloud_commands::enqueue_slot(&state, slot.id).await {
            warnings.push(format!(
                "Slot {} 的云同步队列写入失败：{error}",
                slot.local_port
            ));
        }
    }
    if state.core_running.load(Ordering::Relaxed) {
        if let Err(error) = commands::rebuild_core(&state).await {
            warnings.push(format!("数据已保存，但 Core 应用失败：{error}"));
        }
    }
    Ok(MutationResult {
        value: pending.into_iter().map(|(s, _)| s.id.to_string()).collect(),
        warning: (!warnings.is_empty()).then(|| warnings.join("；")),
    })
}

#[tauri::command]
pub async fn update_slot(
    state: State<'_, ProductState>,
    id: String,
    name: String,
    node_id: String,
) -> Result<MutationResult<()>, String> {
    let id = parse_id(&id)?;
    let node_id = parse_id(&node_id)?;
    let name = validate_slot_name(&name)?;
    let node_exists = commands::subscription_repository(&state)?
        .nodes()
        .map_err(text)?
        .into_iter()
        .any(|node| node.id == node_id && node.present);
    if !node_exists {
        return Err("节点不存在或已从订阅消失".into());
    }
    {
        let mut connection =
            node2socks_storage::open_and_migrate(commands::database_path(&state)?).map_err(text)?;
        let tx = connection.transaction().map_err(text)?;
        let changed = tx.execute(
            "UPDATE proxy_slots SET name=?2,updated_at=strftime('%s','now'),sync_version=sync_version+1 WHERE id=?1",
            params![id.to_string(), name],
        ).map_err(text)?;
        if changed == 0 {
            return Err("Slot 不存在".into());
        }
        tx.execute(
            "UPDATE slot_bindings SET node_id=?2,state='active',updated_at=strftime('%s','now'),sync_version=sync_version+1 WHERE slot_id=?1",
            params![id.to_string(), node_id.to_string()],
        ).map_err(text)?;
        tx.commit().map_err(text)?;
    }
    let mut warnings = Vec::new();
    if let Err(error) = cloud_commands::enqueue_slot(&state, id).await {
        warnings.push(format!("云同步队列写入失败：{error}"));
    }
    if state.core_running.load(Ordering::Relaxed) {
        if let Err(error) = commands::rebuild_core(&state).await {
            warnings.push(format!("数据已保存，但 Core 应用失败：{error}"));
        }
    }
    Ok(MutationResult {
        value: (),
        warning: (!warnings.is_empty()).then(|| warnings.join("；")),
    })
}

#[tauri::command]
pub async fn rename_slot(
    state: State<'_, ProductState>,
    id: String,
    name: String,
) -> Result<(), String> {
    let id = parse_id(&id)?;
    let name = validate_slot_name(&name)?;
    let connection =
        node2socks_storage::open_and_migrate(commands::database_path(&state)?).map_err(text)?;
    let changed = connection
        .execute(
            "UPDATE proxy_slots SET name=?2,updated_at=strftime('%s','now'),sync_version=sync_version+1 WHERE id=?1",
            params![id.to_string(), name],
        )
        .map_err(text)?;
    if changed == 0 {
        return Err("Slot 不存在".into());
    }
    cloud_commands::enqueue_slot(&state, id).await
}

#[tauri::command]
pub async fn batch_delete_slots(
    state: State<'_, ProductState>,
    ids: Vec<String>,
) -> Result<MutationResult<usize>, String> {
    let ids: Vec<Uuid> = ids
        .iter()
        .map(|id| parse_id(id))
        .collect::<Result<_, _>>()?;
    if ids.is_empty() {
        return Ok(MutationResult {
            value: 0,
            warning: None,
        });
    }
    let settings = load_settings(&state)?;
    {
        let mut connection =
            node2socks_storage::open_and_migrate(commands::database_path(&state)?).map_err(text)?;
        let tx = connection.transaction().map_err(text)?;
        let released = now()?;
        let reusable = released + settings.cooldown_hours * 3600;
        for id in &ids {
            let port: Option<u16> = tx
                .query_row(
                    "SELECT local_port FROM proxy_slots WHERE id=?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(text)?;
            let port = port.ok_or("Slot 不存在")?;
            tx.execute("DELETE FROM proxy_slots WHERE id=?1", [id.to_string()])
                .map_err(text)?;
            tx.execute("INSERT INTO port_cooldowns(local_port,released_at,reusable_after) VALUES(?1,?2,?3) ON CONFLICT(local_port) DO UPDATE SET released_at=excluded.released_at,reusable_after=excluded.reusable_after",params![port,released.to_string(),reusable.to_string()]).map_err(text)?;
        }
        tx.commit().map_err(text)?;
    }
    let mut warnings = Vec::new();
    for id in &ids {
        if let Err(error) = cloud_commands::enqueue_tombstone(&state, "slot", *id).await {
            warnings.push(format!("云同步删除队列写入失败：{error}"));
        }
    }
    if state.core_running.load(Ordering::Relaxed) {
        if let Err(error) = commands::rebuild_core(&state).await {
            warnings.push(format!("数据已删除，但 Core 应用失败：{error}"));
        }
    }
    Ok(MutationResult {
        value: ids.len(),
        warning: (!warnings.is_empty()).then(|| warnings.join("；")),
    })
}

#[tauri::command]
pub async fn list_node_views(state: State<'_, ProductState>) -> Result<Vec<NodeView>, String> {
    let subscriptions: HashMap<_, _> = commands::subscription_repository(&state)?
        .list()
        .map_err(text)?
        .into_iter()
        .map(|item| (item.id, item.name))
        .collect();
    let mut bindings: HashMap<Uuid, Vec<BoundSlotView>> = HashMap::new();
    for (slot, binding) in commands::slot_repository(&state)?.list().map_err(text)? {
        if let Some(node_id) = binding.node_id {
            bindings.entry(node_id).or_default().push(BoundSlotView {
                id: slot.id.to_string(),
                port: slot.local_port,
                name: slot.name,
            });
        }
    }
    for slots in bindings.values_mut() {
        slots.sort_by_key(|slot| slot.port);
    }
    let connection =
        node2socks_storage::open_and_migrate(commands::database_path(&state)?).map_err(text)?;
    let mut latency = HashMap::<Uuid, NodeLatencyProbe>::new();
    {
        let mut statement = connection
            .prepare("SELECT node_id,delay_ms,error_message,checked_at FROM node_latency_results")
            .map_err(text)?;
        let rows = statement
            .query_map([], |row| {
                let node_id: String = row.get(0)?;
                Ok(NodeLatencyProbe {
                    node_id: Uuid::parse_str(&node_id).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    delay_ms: row.get(1)?,
                    error: row.get(2)?,
                    checked_at: row.get(3)?,
                })
            })
            .map_err(text)?;
        for row in rows {
            let probe = row.map_err(text)?;
            latency.insert(probe.node_id, probe);
        }
    }
    let health = state.health_results.lock().await.clone();
    Ok(commands::subscription_repository(&state)?
        .nodes()
        .map_err(text)?
        .into_iter()
        .map(|node| {
            let bound_slots = bindings.remove(&node.id).unwrap_or_default();
            let health_result = bound_slots.iter().find_map(|slot| {
                Uuid::parse_str(&slot.id)
                    .ok()
                    .and_then(|id| health.get(&id))
            });
            let probe = latency.get(&node.id);
            NodeView {
                id: node.id.to_string(),
                subscription_id: node.subscription_id.to_string(),
                subscription_name: subscriptions
                    .get(&node.subscription_id)
                    .cloned()
                    .unwrap_or_default(),
                display_name: node.display_name,
                protocol: node.protocol,
                present: node.present,
                bound_slots,
                latency_ms: probe.and_then(|value| value.delay_ms),
                latency_error: probe.and_then(|value| value.error.clone()),
                latency_checked_at: probe.map(|value| value.checked_at),
                exit_ip: health_result.map(|value| value.exit_ip.clone()),
                country: health_result.and_then(|value| value.country.clone()),
            }
        })
        .collect())
}

fn persist_latency(path: &std::path::Path, probe: &NodeLatencyProbe) -> Result<(), String> {
    node2socks_storage::open_and_migrate(path).map_err(text)?.execute(
        "INSERT INTO node_latency_results(node_id,delay_ms,error_code,error_message,checked_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(node_id) DO UPDATE SET delay_ms=excluded.delay_ms,error_code=excluded.error_code,error_message=excluded.error_message,checked_at=excluded.checked_at",
        params![probe.node_id.to_string(), probe.delay_ms, probe.error.as_ref().map(|_| "probe_failed"), probe.error, probe.checked_at],
    ).map_err(text)?;
    Ok(())
}

const LATENCY_TEST_URLS: &[&str] = &[
    "https://www.gstatic.com/generate_204",
    "https://www.google.com/generate_204",
    "https://cp.cloudflare.com/generate_204",
];
const LATENCY_TIMEOUT: Duration = Duration::from_secs(5);
const LATENCY_CONCURRENCY: usize = 6;

async fn probe_node_latency(
    controller: node2socks_core_adapter::controller::MihomoController,
    node_id: Uuid,
    internal_name: String,
) -> NodeLatencyProbe {
    let checked_at = now().unwrap_or_default();
    let mut errors = Vec::new();
    for test_url in LATENCY_TEST_URLS {
        match controller
            .delay(&internal_name, test_url, LATENCY_TIMEOUT)
            .await
        {
            Ok(delay_ms) => {
                return NodeLatencyProbe {
                    node_id,
                    delay_ms: Some(delay_ms),
                    error: None,
                    checked_at,
                };
            }
            Err(error) => errors.push(format!("{}: {}", test_url, error.message)),
        }
    }
    NodeLatencyProbe {
        node_id,
        delay_ms: None,
        error: Some(format!("所有测速地址均失败：{}", errors.join("；"))),
        checked_at,
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyJobView {
    pub job_id: String,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyProgress {
    pub job_id: String,
    pub node_id: Option<String>,
    pub completed: usize,
    pub total: usize,
    pub done: bool,
    pub cancelled: bool,
}

#[tauri::command]
pub async fn start_latency_test(
    app: AppHandle,
    state: State<'_, ProductState>,
    node_ids: Vec<String>,
) -> Result<LatencyJobView, String> {
    if !state.core_running.load(Ordering::Relaxed) {
        return Err("请先启动 Core".into());
    }
    let requested: std::collections::HashSet<Uuid> = node_ids
        .iter()
        .map(|id| parse_id(id))
        .collect::<Result<_, _>>()?;
    if requested.is_empty() {
        return Err("当前筛选结果没有可测速节点".into());
    }
    let nodes: Vec<_> = commands::subscription_repository(&state)?
        .nodes()
        .map_err(text)?
        .into_iter()
        .filter(|node| node.present && requested.contains(&node.id))
        .collect();
    if nodes.len() != requested.len() {
        return Err("所选节点中存在已消失节点".into());
    }
    let controller = state
        .core
        .lock()
        .await
        .as_ref()
        .ok_or("Core 尚未就绪")?
        .controller()
        .await
        .map_err(text)?;
    let job_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    state
        .latency_jobs
        .lock()
        .await
        .insert(job_id, cancel.clone());
    let total = nodes.len();
    let path = commands::database_path(&state)?;
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut completed = 0usize;
        for chunk in nodes.chunks(LATENCY_CONCURRENCY) {
            if cancel.is_cancelled() {
                break;
            }
            let mut tasks = tokio::task::JoinSet::new();
            for node in chunk {
                let controller = controller.clone();
                let node_id = node.id;
                let internal_name = node.internal_name.clone();
                let child = cancel.child_token();
                tasks.spawn(async move {
                    tokio::select! {
                        result = probe_node_latency(controller, node_id, internal_name) => Some(result),
                        _ = child.cancelled() => None,
                    }
                });
            }
            while let Some(joined) = tasks.join_next().await {
                if let Ok(Some(result)) = joined {
                    let _ = persist_latency(&path, &result);
                    completed += 1;
                    let _ = handle.emit(
                        "latency-progress",
                        LatencyProgress {
                            job_id: job_id.to_string(),
                            node_id: Some(result.node_id.to_string()),
                            completed,
                            total,
                            done: false,
                            cancelled: false,
                        },
                    );
                }
            }
        }
        let cancelled = cancel.is_cancelled();
        handle
            .state::<ProductState>()
            .latency_jobs
            .lock()
            .await
            .remove(&job_id);
        let _ = handle.emit(
            "latency-progress",
            LatencyProgress {
                job_id: job_id.to_string(),
                node_id: None,
                completed,
                total,
                done: true,
                cancelled,
            },
        );
    });
    Ok(LatencyJobView {
        job_id: job_id.to_string(),
        total,
    })
}

#[tauri::command]
pub async fn cancel_latency_test(
    state: State<'_, ProductState>,
    job_id: String,
) -> Result<(), String> {
    let id = parse_id(&job_id)?;
    let token = state
        .latency_jobs
        .lock()
        .await
        .get(&id)
        .cloned()
        .ok_or("测速任务已结束")?;
    token.cancel();
    Ok(())
}

#[tauri::command]
pub async fn test_node_latency(
    state: State<'_, ProductState>,
    id: String,
) -> Result<NodeLatencyProbe, String> {
    if !state.core_running.load(Ordering::Relaxed) {
        return Err("请先启动 Core".into());
    }
    let id = parse_id(&id)?;
    let node = commands::subscription_repository(&state)?
        .nodes()
        .map_err(text)?
        .into_iter()
        .find(|node| node.id == id && node.present)
        .ok_or("节点不存在或已从订阅消失")?;
    let controller = state
        .core
        .lock()
        .await
        .as_ref()
        .ok_or("Core 尚未就绪")?
        .controller()
        .await
        .map_err(text)?;
    let result = probe_node_latency(controller, node.id, node.internal_name).await;
    state
        .node_latency_results
        .lock()
        .await
        .insert(result.node_id, result.clone());
    persist_latency(commands::database_path(&state)?.as_path(), &result)?;
    Ok(result)
}

#[tauri::command]
pub async fn test_all_node_latencies(
    state: State<'_, ProductState>,
) -> Result<Vec<NodeLatencyProbe>, String> {
    if !state.core_running.load(Ordering::Relaxed) {
        return Err("请先启动 Core".into());
    }
    let nodes: Vec<_> = commands::subscription_repository(&state)?
        .nodes()
        .map_err(text)?
        .into_iter()
        .filter(|node| node.present)
        .collect();
    let controller = state
        .core
        .lock()
        .await
        .as_ref()
        .ok_or("Core 尚未就绪")?
        .controller()
        .await
        .map_err(text)?;
    let mut results = Vec::with_capacity(nodes.len());
    for chunk in nodes.chunks(LATENCY_CONCURRENCY) {
        let mut tasks = tokio::task::JoinSet::new();
        for node in chunk {
            tasks.spawn(probe_node_latency(
                controller.clone(),
                node.id,
                node.internal_name.clone(),
            ));
        }
        while let Some(result) = tasks.join_next().await {
            results.push(result.map_err(text)?);
        }
    }
    let path = commands::database_path(&state)?;
    for result in &results {
        persist_latency(path.as_path(), result)?;
    }
    let mut cache = state.node_latency_results.lock().await;
    for result in &results {
        cache.insert(result.node_id, result.clone());
    }
    Ok(results)
}

#[tauri::command]
pub async fn check_node(
    state: State<'_, ProductState>,
    id: String,
) -> Result<HealthResult, String> {
    let id = parse_id(&id)?;
    if !state.core_running.load(Ordering::Relaxed) {
        return Err("请先启动 Core".into());
    }
    let (slot, binding) = commands::slot_repository(&state)?
        .list()
        .map_err(text)?
        .into_iter()
        .find(|(_, b)| b.node_id == Some(id))
        .ok_or("该节点尚未绑定 Slot；为避免改变已有出口，请先创建或绑定 Slot")?;
    if binding.state != SlotBindingState::Active {
        return Err("该节点所在 Slot 当前已阻断".into());
    }
    let health = HealthChecker::public_ip(Duration::from_secs(8))
        .check_socks(slot.local_port, &CancellationToken::new())
        .await
        .map_err(text)?;
    state
        .health_results
        .lock()
        .await
        .insert(slot.id, health.clone());
    Ok(health)
}

#[tauri::command]
pub fn data_directory(state: State<'_, ProductState>) -> Result<String, String> {
    Ok(commands::database_path(&state)?
        .parent()
        .ok_or("无法定位数据目录")?
        .display()
        .to_string())
}

#[tauri::command]
pub fn open_data_directory(app: AppHandle, state: State<'_, ProductState>) -> Result<(), String> {
    let path = commands::database_path(&state)?
        .parent()
        .ok_or("无法定位数据目录")?
        .to_path_buf();
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(text)?;
    }
    let _ = app;
    Ok(())
}

pub(crate) fn desired_outbound_interface(state: &ProductState) -> Result<Option<String>, String> {
    let settings = load_settings(state)?;
    match settings.outbound_mode.as_str() {
        "system" => Ok(None),
        "auto" => Ok(node2socks_diagnostics::inspect_windows()
            .ok()
            .and_then(|r| r.recommended_interface)),
        "manual" => Ok(settings.outbound_interface),
        _ => Err("出站模式无效".into()),
    }
}

pub(crate) fn synced_settings_payload(
    state: &ProductState,
) -> Result<(serde_json::Value, u64), String> {
    let settings = load_settings(state)?;
    let connection =
        node2socks_storage::open_and_migrate(commands::database_path(state)?).map_err(text)?;
    let revision: Option<u64> = connection
        .query_row(
            "SELECT max(sync_version) FROM app_settings WHERE scope='synced'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(text)?
        .flatten();
    Ok((
        serde_json::json!({"theme":settings.theme,"density":settings.density,"portStart":settings.port_start,"portEnd":settings.port_end,"cooldownHours":settings.cooldown_hours}),
        revision.unwrap_or(0),
    ))
}

pub(crate) fn apply_synced_settings(
    state: &ProductState,
    value: &serde_json::Value,
) -> Result<(), String> {
    let mut settings = load_settings(state)?;
    if let Some(item) = value.get("theme").and_then(|v| v.as_str()) {
        settings.theme = item.to_owned()
    }
    if let Some(item) = value.get("density").and_then(|v| v.as_str()) {
        settings.density = item.to_owned()
    }
    if let Some(item) = value.get("portStart").and_then(|v| v.as_u64()) {
        settings.port_start = u16::try_from(item).map_err(text)?
    }
    if let Some(item) = value.get("portEnd").and_then(|v| v.as_u64()) {
        settings.port_end = u16::try_from(item).map_err(text)?
    }
    if let Some(item) = value.get("cooldownHours").and_then(|v| v.as_u64()) {
        settings.cooldown_hours = item
    }
    validate_settings(&settings)?;
    let values = [
        ("theme", serde_json::json!(settings.theme)),
        ("density", serde_json::json!(settings.density)),
        ("port_start", serde_json::json!(settings.port_start)),
        ("port_end", serde_json::json!(settings.port_end)),
        ("cooldown_hours", serde_json::json!(settings.cooldown_hours)),
    ];
    let mut connection =
        node2socks_storage::open_and_migrate(commands::database_path(state)?).map_err(text)?;
    let tx = connection.transaction().map_err(text)?;
    for (key, value) in values {
        tx.execute("INSERT INTO app_settings(key,value_json,scope,updated_at,sync_version) VALUES(?1,?2,'synced',strftime('%s','now'),0) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json,scope='synced',updated_at=excluded.updated_at,sync_version=app_settings.sync_version+1",params![key,value.to_string()]).map_err(text)?;
    }
    tx.commit().map_err(text)
}

fn detail(item: SubscriptionRecord) -> SubscriptionDetail {
    SubscriptionDetail {
        id: item.id.to_string(),
        name: item.name,
        url: item.url,
        enabled: item.enabled,
        refresh_interval_sec: item.refresh_interval_sec,
        next_refresh_at: item.next_refresh_at,
        last_success_at: item.last_success_at,
        last_error: item.last_error,
        download_mode: mode_name(&item.download_mode).into(),
        user_agent: item.user_agent,
        proxy_url: item.proxy_url,
        headers: item
            .headers
            .into_iter()
            .map(|(name, value)| HeaderEntry { name, value })
            .collect(),
    }
}
fn validate_subscription(input: &SubscriptionInput) -> Result<(), String> {
    if input.name.trim().is_empty() {
        return Err("订阅名称不能为空".into());
    }
    if input.refresh_interval_sec != 0 && !(60..=2_592_000).contains(&input.refresh_interval_sec) {
        return Err("刷新周期必须为手动（0），或在 60 秒到 30 天之间".into());
    }
    let url = url::Url::parse(input.url.trim()).map_err(text)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("订阅仅支持 HTTP/HTTPS".into());
    }
    let mode = mode(&input.download_mode)?;
    match mode {
        DownloadMode::Direct => {
            if clean(input.proxy_url.clone()).is_some() {
                return Err("直连下载不能填写代理地址".into());
            }
        }
        DownloadMode::CustomHttp => {
            validate_proxy_url(input.proxy_url.as_deref(), &["http", "https"])?
        }
        DownloadMode::CustomSocks5 => {
            validate_proxy_url(input.proxy_url.as_deref(), &["socks5", "socks5h"])?
        }
    }
    Ok(())
}
fn validate_proxy_url(value: Option<&str>, schemes: &[&str]) -> Result<(), String> {
    let raw = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("自定义下载代理必须填写地址")?;
    let parsed = url::Url::parse(raw).map_err(|_| "自定义下载代理地址无效".to_owned())?;
    if !schemes.contains(&parsed.scheme())
        || parsed.host_str().is_none()
        || parsed.port_or_known_default().is_none()
    {
        return Err("自定义下载代理协议或地址无效".into());
    }
    Ok(())
}

fn validate_slot_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("槽位名称不能为空".into());
    }
    if value.chars().count() > 80 {
        return Err("槽位名称不能超过 80 个字符".into());
    }
    if value.chars().any(char::is_control) {
        return Err("槽位名称不能包含换行或控制字符".into());
    }
    Ok(value.to_owned())
}
fn mode(value: &str) -> Result<DownloadMode, String> {
    match value {
        "direct" => Ok(DownloadMode::Direct),
        "custom_http" => Ok(DownloadMode::CustomHttp),
        "custom_socks5" => Ok(DownloadMode::CustomSocks5),
        _ => Err("下载模式无效".into()),
    }
}
fn mode_name(value: &DownloadMode) -> &'static str {
    match value {
        DownloadMode::Direct => "direct",
        DownloadMode::CustomHttp => "custom_http",
        DownloadMode::CustomSocks5 => "custom_socks5",
    }
}
fn clean(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}
fn parse_id(value: &str) -> Result<Uuid, String> {
    Uuid::parse_str(value).map_err(text)
}
fn decode<T: for<'de> Deserialize<'de>>(raw: &str) -> Result<T, String> {
    serde_json::from_str(raw).map_err(text)
}
fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(text)
}
fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_validation_rejects_invalid_ranges() {
        let value = AppSettings {
            port_start: 22000,
            port_end: 21000,
            ..AppSettings::default()
        };
        assert!(validate_settings(&value).is_err());
    }
    #[test]
    fn settings_defaults_are_compact_and_local() {
        let value = AppSettings::default();
        assert_eq!(value.theme, "system");
        assert_eq!(value.density, "compact");
        assert_eq!(value.port_start, 21000);
    }
    #[test]
    fn slot_name_validation_trims_and_rejects_unsafe_values() {
        assert_eq!(validate_slot_name("  店铺 A  ").unwrap(), "店铺 A");
        assert!(validate_slot_name("  ").is_err());
        assert!(validate_slot_name("店铺\nA").is_err());
        assert!(validate_slot_name(&"a".repeat(81)).is_err());
    }
}
