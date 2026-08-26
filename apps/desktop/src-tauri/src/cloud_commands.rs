use crate::backend::ProductState;
use node2socks_cloud_sync::{CloudClient, Outbox, PullEvent};
use node2socks_crypto::{Envelope, SecretKey, decrypt};
use node2socks_domain::{ProxySlot, SlotBinding, SlotBindingState};
use node2socks_slot_manager::{
    PortAllocator, PortRange, SlotRepository, SqliteSlotRepository, SystemPortProbe,
};
use node2socks_subscriptions::{SubscriptionRecord, SubscriptionRepository, SubscriptionService};
use rusqlite::params;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::State;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tauri::command]
pub async fn cloud_push_local(state: State<'_, ProductState>) -> Result<usize, String> {
    let (base_url, token, vault_key) = cloud_snapshot(&state).await?;
    let path = database_path(&state)?;
    let repository = SubscriptionRepository::new(
        node2socks_storage::open_and_migrate(&path).map_err(text)?,
        local_key(&state)?,
    );
    let outbox = Outbox::new(node2socks_storage::open_and_migrate(&path).map_err(text)?);
    let nodes_by_id: std::collections::HashMap<Uuid, String> = repository
        .nodes()
        .map_err(text)?
        .into_iter()
        .map(|node| (node.id, node.stable_key))
        .collect();
    for item in repository.list().map_err(text)? {
        let cache = repository.cached_payload(item.id).map_err(text)?;
        outbox
            .enqueue_json(
                &vault_key,
                "subscription",
                item.id,
                item.revision,
                false,
                &serde_json::json!({"subscription":item,"cache":cache}),
            )
            .map_err(text)?;
    }
    for (slot, binding) in slot_repository(&path)?.list().map_err(text)? {
        outbox
            .enqueue_json(
                &vault_key,
                "slot",
                slot.id,
                slot.revision.max(binding.revision),
                false,
                &serde_json::json!({"slot":slot,"binding":binding,"binding_node_stable_key":binding.node_id.and_then(|id|nodes_by_id.get(&id).cloned())}),
            )
            .map_err(text)?;
    }
    let (settings, revision) = crate::advanced_commands::synced_settings_payload(&state)?;
    outbox
        .enqueue_json(
            &vault_key,
            "settings",
            Uuid::nil(),
            revision,
            false,
            &settings,
        )
        .map_err(text)?;
    CloudClient::new(&base_url, is_local(&base_url))
        .map_err(text)?
        .push_outbox(&token, &outbox, &CancellationToken::new())
        .await
        .map_err(text)
}

#[tauri::command]
pub async fn cloud_pull_merge(state: State<'_, ProductState>) -> Result<usize, String> {
    if state.core_running.load(Ordering::Relaxed) {
        return Err("合并云端配置前请先停止 Core；这样可以可靠检测并保留每个固定端口".into());
    }
    let (base_url, token, vault_key) = refresh_cloud_session(&state).await?;
    let cursor = state
        .cloud
        .lock()
        .await
        .as_ref()
        .map(|session| session.cursor)
        .unwrap_or(0);
    let events = CloudClient::new(&base_url, is_local(&base_url))
        .map_err(text)?
        .pull(&token, cursor, &CancellationToken::new())
        .await
        .map_err(text)?;
    let path = database_path(&state)?;
    let values = decrypt_events(&vault_key, events)?;
    for (event, value) in values
        .iter()
        .filter(|(event, _)| event.record_type == "subscription")
    {
        apply_subscription(&state, &path, event, value).await?;
    }
    for (event, value) in values
        .iter()
        .filter(|(event, _)| event.record_type == "slot")
    {
        apply_slot(&state, &path, event, value)?;
    }
    for (_, value) in values
        .iter()
        .filter(|(event, _)| event.record_type == "settings")
    {
        crate::advanced_commands::apply_synced_settings(&state, value)?;
    }
    if !values.is_empty() {
        let service = SubscriptionService::new(
            SubscriptionRepository::new(
                node2socks_storage::open_and_migrate(&path).map_err(text)?,
                local_key(&state)?,
            ),
            state.bridge.clone(),
            4,
        )
        .map_err(text)?;
        service.restore_bridge().await.map_err(text)?;
        let next = values
            .iter()
            .map(|(event, _)| event.cursor)
            .max()
            .unwrap_or(cursor);
        if let Some(session) = state.cloud.lock().await.as_mut() {
            session.cursor = next;
        }
        persist_cloud_cursor(&path, &base_url, next)?;
    }
    Ok(values.len())
}

pub async fn enqueue_settings(state: &ProductState) -> Result<(), String> {
    let session_key = state
        .cloud
        .lock()
        .await
        .as_ref()
        .map(|session| session.vault_key.clone());
    let key = session_key.or_else(|| state.sync_key.lock().ok().and_then(|key| key.clone()));
    let Some(key) = key else { return Ok(()) };
    let path = database_path(state)?;
    let (value, revision) = crate::advanced_commands::synced_settings_payload(state)?;
    Outbox::new(node2socks_storage::open_and_migrate(path).map_err(text)?)
        .enqueue_json(&key, "settings", Uuid::nil(), revision, false, &value)
        .map_err(text)
}

pub async fn enqueue_subscription(state: &ProductState, id: Uuid) -> Result<(), String> {
    let session_key = state
        .cloud
        .lock()
        .await
        .as_ref()
        .map(|session| session.vault_key.clone());
    let key = session_key.or_else(|| state.sync_key.lock().ok().and_then(|key| key.clone()));
    let Some(key) = key else { return Ok(()) };
    let path = database_path(state)?;
    let repository = SubscriptionRepository::new(
        node2socks_storage::open_and_migrate(&path).map_err(text)?,
        local_key(state)?,
    );
    let item = repository
        .get(id)
        .map_err(text)?
        .ok_or_else(|| "订阅不存在".to_owned())?;
    let cache = repository.cached_payload(id).map_err(text)?;
    Outbox::new(node2socks_storage::open_and_migrate(path).map_err(text)?)
        .enqueue_json(
            &key,
            "subscription",
            id,
            item.revision,
            false,
            &serde_json::json!({"subscription":item,"cache":cache}),
        )
        .map_err(text)
}

pub async fn enqueue_slot(state: &ProductState, id: Uuid) -> Result<(), String> {
    let session_key = state
        .cloud
        .lock()
        .await
        .as_ref()
        .map(|session| session.vault_key.clone());
    let key = session_key.or_else(|| state.sync_key.lock().ok().and_then(|key| key.clone()));
    let Some(key) = key else { return Ok(()) };
    let path = database_path(state)?;
    let (slot, binding) = slot_repository(&path)?
        .list()
        .map_err(text)?
        .into_iter()
        .find(|(slot, _)| slot.id == id)
        .ok_or_else(|| "Slot 不存在".to_owned())?;
    let binding_node_stable_key = match binding.node_id {
        Some(node_id) => node_stable_key(state, &path, node_id)?,
        None => None,
    };
    Outbox::new(node2socks_storage::open_and_migrate(&path).map_err(text)?)
        .enqueue_json(
            &key,
            "slot",
            id,
            slot.revision.max(binding.revision),
            false,
            &serde_json::json!({"slot":slot,"binding":binding,"binding_node_stable_key":binding_node_stable_key}),
        )
        .map_err(text)
}

pub async fn enqueue_tombstone(
    state: &ProductState,
    record_type: &str,
    id: Uuid,
) -> Result<(), String> {
    let session_key = state
        .cloud
        .lock()
        .await
        .as_ref()
        .map(|session| session.vault_key.clone());
    let key = session_key.or_else(|| state.sync_key.lock().ok().and_then(|key| key.clone()));
    if let Some(key) = key {
        Outbox::new(node2socks_storage::open_and_migrate(database_path(state)?).map_err(text)?)
            .enqueue_json(&key, record_type, id, 0, true, &serde_json::json!({}))
            .map_err(text)?;
    }
    Ok(())
}

fn decrypt_events(
    key: &SecretKey,
    events: Vec<PullEvent>,
) -> Result<Vec<(PullEvent, Value)>, String> {
    events
        .into_iter()
        .map(|event| {
            if event.deleted {
                return Ok((event, Value::Null));
            }
            let base_version = event.version.saturating_sub(1);
            let aad = format!("{}:{}:{base_version}", event.record_type, event.record_id);
            let envelope = serde_json::to_vec(&Envelope {
                version: event.aad_version as u8,
                nonce: event.nonce.clone(),
                ciphertext: event.ciphertext.clone(),
            })
            .map_err(text)?;
            let value =
                serde_json::from_slice(&decrypt(key, &envelope, aad.as_bytes()).map_err(text)?)
                    .map_err(text)?;
            Ok((event, value))
        })
        .collect()
}

async fn apply_subscription(
    state: &ProductState,
    path: &PathBuf,
    event: &PullEvent,
    value: &Value,
) -> Result<(), String> {
    let id = Uuid::parse_str(&event.record_id).map_err(text)?;
    let repository = SubscriptionRepository::new(
        node2socks_storage::open_and_migrate(path).map_err(text)?,
        local_key(state)?,
    );
    if event.deleted {
        let node_ids: HashSet<_> = repository
            .nodes()
            .map_err(text)?
            .into_iter()
            .filter(|node| node.subscription_id == id)
            .map(|node| node.id)
            .collect();
        let ports: Vec<_> = slot_repository(path)?
            .list()
            .map_err(text)?
            .into_iter()
            .filter(|(_, binding)| binding.node_id.is_some_and(|node| node_ids.contains(&node)))
            .map(|(slot, _)| slot.local_port)
            .collect();
        if !ports.is_empty() {
            return Err(format!(
                "云端删除的订阅仍被端口 {} 使用；已停止合并以保持 fail-closed 身份",
                ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        repository.delete(id).map_err(text)?;
    } else {
        let item: SubscriptionRecord = serde_json::from_value(
            value
                .get("subscription")
                .cloned()
                .ok_or_else(|| "云订阅记录缺少 subscription".to_owned())?,
        )
        .map_err(text)?;
        repository.upsert(&item).map_err(text)?;
        if let Some(cache) = value.get("cache").filter(|v| !v.is_null()) {
            let cache: Vec<u8> = serde_json::from_value(cache.clone()).map_err(text)?;
            let parsed =
                node2socks_subscriptions::detect_and_normalize(item.id, &cache).map_err(text)?;
            repository
                .apply_nodes(
                    item.id,
                    &parsed.nodes,
                    &cache,
                    &format!("{:?}", parsed.format).to_ascii_lowercase(),
                )
                .map_err(text)?;
        }
    }
    record_version(path, event)
}

fn apply_slot(
    state: &ProductState,
    path: &PathBuf,
    event: &PullEvent,
    value: &Value,
) -> Result<(), String> {
    let id = Uuid::parse_str(&event.record_id).map_err(text)?;
    let repository = slot_repository(path)?;
    let existing = repository.list().map_err(text)?;
    if event.deleted {
        if existing.iter().any(|(slot, _)| slot.id == id) {
            repository
                .delete_with_cooldown(id, Duration::from_secs(24 * 60 * 60))
                .map_err(text)?;
        }
        return record_version(path, event);
    }
    let slot: ProxySlot = serde_json::from_value(
        value
            .get("slot")
            .cloned()
            .ok_or_else(|| "云 Slot 记录缺少 slot".to_owned())?,
    )
    .map_err(text)?;
    let mut binding: SlotBinding = serde_json::from_value(
        value
            .get("binding")
            .cloned()
            .ok_or_else(|| "云 Slot 记录缺少 binding".to_owned())?,
    )
    .map_err(text)?;
    if let Some(stable_key) = value
        .get("binding_node_stable_key")
        .and_then(|v| v.as_str())
    {
        binding.node_id = Some(resolve_node_by_stable_key(
            path,
            &local_key(state)?,
            stable_key,
        )?);
    }
    ensure_no_saved_port_conflict(&existing, &slot)?;
    let same_port = existing
        .iter()
        .any(|(saved, _)| saved.id == slot.id && saved.local_port == slot.local_port);
    if !same_port {
        PortAllocator::new(
            PortRange {
                start: 1,
                end: u16::MAX,
            },
            Duration::ZERO,
            SystemPortProbe,
        )
        .map_err(text)?
        .validate_stable_port(slot.local_port)
        .map_err(|conflict| {
            format!(
                "端口 {} 已被 {} (PID {}) 占用；不会自动改号",
                conflict.port,
                conflict.process_name.unwrap_or_else(|| "未知进程".into()),
                conflict
                    .pid
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "未知".into())
            )
        })?;
    }
    let connection = node2socks_storage::open_and_migrate(path).map_err(text)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(text)?
        .as_secs()
        .to_string();
    connection.execute("INSERT INTO proxy_slots(id,name,local_port,listen_host,enabled,created_at,updated_at,sync_version) VALUES(?1,?2,?3,'127.0.0.1',?4,?5,?5,?6) ON CONFLICT(id) DO UPDATE SET name=excluded.name,local_port=excluded.local_port,enabled=excluded.enabled,updated_at=excluded.updated_at,sync_version=excluded.sync_version",params![slot.id.to_string(),slot.name,slot.local_port,slot.enabled,now,slot.revision]).map_err(text)?;
    connection.execute("INSERT INTO slot_bindings(slot_id,node_id,state,updated_at,sync_version) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(slot_id) DO UPDATE SET node_id=excluded.node_id,state=excluded.state,updated_at=excluded.updated_at,sync_version=excluded.sync_version",params![slot.id.to_string(),binding.node_id.map(|id|id.to_string()),binding_state(binding.state),now,binding.revision]).map_err(text)?;
    record_version(path, event)
}

fn record_version(path: &PathBuf, event: &PullEvent) -> Result<(), String> {
    node2socks_storage::open_and_migrate(path).map_err(text)?.execute("INSERT INTO sync_versions(record_type,record_id,cloud_version) VALUES(?1,?2,?3) ON CONFLICT(record_type,record_id) DO UPDATE SET cloud_version=excluded.cloud_version",params![event.record_type,event.record_id,event.version]).map_err(text)?;
    Ok(())
}
fn slot_repository(path: &PathBuf) -> Result<SqliteSlotRepository, String> {
    Ok(SqliteSlotRepository::new(
        node2socks_storage::open_and_migrate(path).map_err(text)?,
    ))
}
fn database_path(state: &ProductState) -> Result<PathBuf, String> {
    state
        .database
        .lock()
        .map_err(text)?
        .clone()
        .ok_or_else(|| "数据库未初始化".into())
}
fn local_key(state: &ProductState) -> Result<SecretKey, String> {
    state
        .master_key
        .lock()
        .map_err(text)?
        .clone()
        .ok_or_else(|| "主密钥未初始化".into())
}
async fn cloud_snapshot(state: &ProductState) -> Result<(String, String, SecretKey), String> {
    refresh_cloud_session(state).await
}
pub(crate) async fn refresh_cloud_session(
    state: &ProductState,
) -> Result<(String, String, SecretKey), String> {
    let (base_url, refresh_token) = {
        let session = state.cloud.lock().await;
        let session = session
            .as_ref()
            .ok_or_else(|| "请先登录云同步".to_owned())?;
        (session.base_url.clone(), session.refresh_token.clone())
    };
    let tokens = CloudClient::new(&base_url, is_local(&base_url))
        .map_err(text)?
        .refresh(&refresh_token, &CancellationToken::new())
        .await
        .map_err(text)?;
    let mut session = state.cloud.lock().await;
    let session = session
        .as_mut()
        .ok_or_else(|| "云同步会话已结束".to_owned())?;
    session.access_token = tokens.access_token;
    session.refresh_token = tokens.refresh_token;
    session.device_id = tokens.device_id;
    Ok((
        base_url,
        session.access_token.clone(),
        session.vault_key.clone(),
    ))
}
fn is_local(value: &str) -> bool {
    url::Url::parse(value)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| matches!(host, "localhost" | "127.0.0.1"))
        })
        .unwrap_or(false)
}
fn binding_state(value: SlotBindingState) -> &'static str {
    match value {
        SlotBindingState::Active => "active",
        SlotBindingState::Orphaned => "orphaned",
        SlotBindingState::Unbound => "unbound",
        SlotBindingState::Blocked => "blocked",
        SlotBindingState::Error => "error",
    }
}
fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn persist_cloud_cursor(path: &PathBuf, base_url: &str, cursor: u64) -> Result<(), String> {
    node2socks_storage::open_and_migrate(path)
        .map_err(text)?
        .execute(
            "UPDATE cloud_profiles SET last_cursor=?1, updated_at=strftime('%s','now') WHERE base_url=?2 AND is_active=1",
            params![cursor, base_url],
        )
        .map_err(text)?;
    Ok(())
}
fn node_stable_key(
    state: &ProductState,
    path: &PathBuf,
    id: Uuid,
) -> Result<Option<String>, String> {
    Ok(SubscriptionRepository::new(
        node2socks_storage::open_and_migrate(path).map_err(text)?,
        local_key(state)?,
    )
    .nodes()
    .map_err(text)?
    .into_iter()
    .find(|node| node.id == id)
    .map(|node| node.stable_key))
}
fn resolve_node_by_stable_key(
    path: &PathBuf,
    key: &SecretKey,
    stable_key: &str,
) -> Result<Uuid, String> {
    SubscriptionRepository::new(
        node2socks_storage::open_and_migrate(path).map_err(text)?,
        key.clone(),
    )
    .nodes()
    .map_err(text)?
    .into_iter()
    .find(|node| node.stable_key == stable_key && node.present)
    .map(|node| node.id)
    .ok_or_else(|| format!("云 Slot 绑定节点 {stable_key} 尚未在本地目录中出现"))
}
fn ensure_no_saved_port_conflict(
    existing: &[(ProxySlot, SlotBinding)],
    slot: &ProxySlot,
) -> Result<(), String> {
    if existing
        .iter()
        .any(|(saved, _)| saved.local_port == slot.local_port && saved.id != slot.id)
    {
        return Err(format!(
            "云 Slot {} 的端口 {} 与本地其他 Slot 冲突；不会自动改号",
            slot.name, slot.local_port
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use node2socks_subscriptions::{DownloadMode, SubscriptionRecord};
    use tempfile::tempdir;

    #[test]
    fn stable_key_maps_cloud_binding_to_local_node_uuid() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("app.db");
        let key = SecretKey::random();
        let subscription_id = Uuid::new_v4();
        let repository = SubscriptionRepository::new(
            node2socks_storage::open_and_migrate(&path).unwrap(),
            key.clone(),
        );
        repository
            .upsert(&SubscriptionRecord {
                id: subscription_id,
                name: "restore".into(),
                url: "https://example.test/sub".into(),
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
            })
            .unwrap();
        let parsed = node2socks_subscriptions::detect_and_normalize(
            subscription_id,
            b"proxies:\n  - {name: renamed, type: ss, server: 1.2.3.4, port: 443, password: p, cipher: aes-128-gcm}\n",
        )
        .unwrap();
        repository
            .apply_nodes(subscription_id, &parsed.nodes, b"payload", "provider_yaml")
            .unwrap();
        let local = repository.nodes().unwrap().remove(0);
        let remote_uuid = Uuid::new_v4();
        assert_ne!(local.id, remote_uuid);
        assert_eq!(
            resolve_node_by_stable_key(&path, &key, &local.stable_key).unwrap(),
            local.id
        );
    }

    #[test]
    fn cloud_restore_rejects_saved_port_collision_without_renumbering() {
        let existing_slot = ProxySlot::new("local", 21_001).unwrap();
        let existing_binding = SlotBinding {
            slot_id: existing_slot.id,
            node_id: None,
            state: SlotBindingState::Unbound,
            revision: 0,
        };
        let cloud_slot = ProxySlot::new("cloud", 21_001).unwrap();
        let error =
            ensure_no_saved_port_conflict(&[(existing_slot, existing_binding)], &cloud_slot)
                .unwrap_err();
        assert!(error.contains("不会自动改号"));
        assert_eq!(cloud_slot.local_port, 21_001);
    }
}
