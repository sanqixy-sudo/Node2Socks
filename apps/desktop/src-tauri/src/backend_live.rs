use crate::{cloud_commands, commands};
use node2socks_cloud_sync::{CloudClient, CloudDevice};
use node2socks_core_adapter::{
    ProxyCore,
    mihomo::{CrashMonitor, MihomoManager},
};
use node2socks_crypto::{
    SecretKey, decrypt,
    dpapi::{protect_key, unprotect_key},
    encrypt,
};
use node2socks_diagnostics::inspect_windows;
use node2socks_domain::SlotBindingState;
use node2socks_recovery::{
    backup_database, export_diagnostics, restore_database as restore_database_file,
};
use node2socks_runtime_service::HealthResult;
use node2socks_slot_manager::{SlotRepository, SqliteSlotRepository};
use node2socks_subscriptions::{
    ProviderBridge, ProviderBridgeHandle, SubscriptionRepository, SubscriptionService,
};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::async_runtime::JoinHandle;
use tauri::{
    AppHandle, Manager, RunEvent, State, WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub struct ProductState {
    pub master_key: Mutex<Option<SecretKey>>,
    pub database: Mutex<Option<PathBuf>>,
    pub app_handle: Mutex<Option<AppHandle>>,
    pub bridge: ProviderBridge,
    pub bridge_handle: AsyncMutex<Option<ProviderBridgeHandle>>,
    pub core: AsyncMutex<Option<Arc<MihomoManager>>>,
    pub crash_monitor: AsyncMutex<Option<CrashMonitor>>,
    pub core_running: AtomicBool,
    /// Session-stable localhost port of the dedicated subscription-download
    /// SOCKS listener; chosen on Core start when a node-mode subscription exists.
    pub download_port: Mutex<Option<u16>>,
    pub cloud: AsyncMutex<Option<CloudSession>>,
    pub sync_key: Mutex<Option<SecretKey>>,
    pub refresh_cancel: CancellationToken,
    pub refresh_task: AsyncMutex<Option<JoinHandle<()>>>,
    pub health_results: AsyncMutex<HashMap<uuid::Uuid, HealthResult>>,
    pub node_latency_results: AsyncMutex<HashMap<uuid::Uuid, NodeLatencyProbe>>,
    pub latency_jobs: AsyncMutex<HashMap<uuid::Uuid, CancellationToken>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodeLatencyProbe {
    pub node_id: uuid::Uuid,
    pub delay_ms: Option<u64>,
    pub error: Option<String>,
    pub checked_at: u64,
}

pub(crate) struct CloudSession {
    pub base_url: String,
    pub access_token: String,
    pub refresh_token: String,
    pub device_id: String,
    pub vault_key: SecretKey,
    pub cursor: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Dashboard {
    version: &'static str,
    core_running: bool,
    local_mode_available: bool,
    slots: Vec<SlotSummary>,
    subscriptions: Vec<commands::SubscriptionView>,
    diagnostics: DiagnosticSummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SlotSummary {
    id: String,
    name: String,
    port: u16,
    node_name: Option<String>,
    node_id: Option<String>,
    state: String,
    latency_ms: Option<u64>,
    exit_ip: Option<String>,
    country: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticSummary {
    core: String,
    clash: String,
    system_proxy: String,
    tun: String,
    outbound_adapter: String,
    warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotSection<T> {
    data: Option<T>,
    error: Option<String>,
}

impl<T> SnapshotSection<T> {
    fn from_result(result: Result<T, String>) -> Self {
        match result {
            Ok(data) => Self {
                data: Some(data),
                error: None,
            },
            Err(error) => Self {
                data: None,
                error: Some(error),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    dashboard: SnapshotSection<Dashboard>,
    nodes: SnapshotSection<Vec<crate::advanced_commands::NodeView>>,
    settings: SnapshotSection<crate::advanced_commands::AppSettings>,
    cloud: SnapshotSection<CloudStatusView>,
}

#[tauri::command]
async fn app_snapshot(state: State<'_, ProductState>) -> Result<AppSnapshot, String> {
    let dashboard = dashboard_snapshot(state.clone()).await;
    let nodes = crate::advanced_commands::list_node_views(state.clone()).await;
    let settings = crate::advanced_commands::get_settings(state.clone());
    let cloud = cloud_status(state).await;
    Ok(AppSnapshot {
        dashboard: SnapshotSection::from_result(dashboard),
        nodes: SnapshotSection::from_result(nodes),
        settings: SnapshotSection::from_result(settings),
        cloud: SnapshotSection::from_result(cloud),
    })
}

#[tauri::command]
async fn dashboard_snapshot(state: State<'_, ProductState>) -> Result<Dashboard, String> {
    let path = database_path(&state)?;
    let key = state
        .master_key
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("master key not ready")?;
    let subscriptions = commands::subscription_views(&state)?;
    let node_names: HashMap<_, _> = SubscriptionRepository::new(
        node2socks_storage::open_and_migrate(&path).map_err(text)?,
        key.clone(),
    )
    .nodes()
    .map_err(text)?
    .into_iter()
    .map(|node| (node.id, node.display_name))
    .collect();
    let core_running = match state.core.lock().await.as_ref() {
        Some(manager) => manager.health().await.is_ok(),
        None => false,
    };
    state.core_running.store(core_running, Ordering::Relaxed);
    let health_results = state.health_results.lock().await.clone();
    let slots =
        SqliteSlotRepository::new(node2socks_storage::open_and_migrate(&path).map_err(text)?)
            .list()
            .map_err(text)?
            .into_iter()
            .map(|(slot, binding)| SlotSummary {
                id: slot.id.to_string(),
                name: slot.name,
                port: slot.local_port,
                node_name: binding.node_id.and_then(|id| node_names.get(&id).cloned()),
                node_id: binding.node_id.map(|id| id.to_string()),
                state: binding_state(
                    if !core_running && binding.state == SlotBindingState::Active {
                        SlotBindingState::Blocked
                    } else {
                        binding.state
                    },
                )
                .into(),
                latency_ms: health_results.get(&slot.id).map(|health| health.latency_ms),
                exit_ip: health_results
                    .get(&slot.id)
                    .map(|health| health.exit_ip.clone()),
                country: health_results
                    .get(&slot.id)
                    .and_then(|health| health.country.clone()),
            })
            .collect();
    let report = inspect_windows().ok();
    let diagnostics = report
        .map(|value| DiagnosticSummary {
            core: if core_running {
                "运行中"
            } else {
                "已停止"
            }
            .into(),
            clash: format!("{:?}", value.clash_mode),
            system_proxy: value.system_proxy.unwrap_or_else(|| "未启用".into()),
            tun: if value.tun_adapters.is_empty() {
                "未检测".into()
            } else {
                format!("疑似启用 · {}", value.tun_adapters.join(", "))
            },
            outbound_adapter: value
                .recommended_interface
                .unwrap_or_else(|| "跟随系统路由".into()),
            warning: value.warning,
        })
        .unwrap_or(DiagnosticSummary {
            core: if core_running {
                "运行中"
            } else {
                "已停止"
            }
            .into(),
            clash: "未检测".into(),
            system_proxy: "未启用".into(),
            tun: "未检测".into(),
            outbound_adapter: "跟随系统路由".into(),
            warning: None,
        });
    Ok(Dashboard {
        version: env!("CARGO_PKG_VERSION"),
        core_running,
        local_mode_available: true,
        slots,
        subscriptions,
        diagnostics,
    })
}

#[tauri::command]
fn create_backup(state: State<'_, ProductState>, destination: String) -> Result<String, String> {
    backup_database(&database_path(&state)?, &PathBuf::from(destination))
        .map(|p| p.display().to_string())
        .map_err(text)
}

#[tauri::command]
async fn restore_backup(state: State<'_, ProductState>, source: String) -> Result<(), String> {
    let database = database_path(&state)?;
    let source = PathBuf::from(source);
    if !source.is_file() {
        return Err("所选备份文件不存在".into());
    }
    let was_running = state.core_running.load(Ordering::Relaxed);
    if was_running {
        commands::stop_core_inner(&state).await?;
    }
    let backup_dir = database
        .parent()
        .ok_or_else(|| "无法定位数据库目录".to_owned())?
        .join("backups");
    backup_database(&database, &backup_dir).map_err(text)?;
    restore_database_file(&source, &database).map_err(text)?;
    node2socks_storage::open_and_migrate(&database).map_err(text)?;
    if was_running {
        commands::start_core_inner(&state).await?;
    }
    Ok(())
}

#[tauri::command]
fn diagnostic_export(state: State<'_, ProductState>, destination: String) -> Result<(), String> {
    let report = inspect_windows()
        .map(|v| format!("{v:#?}"))
        .unwrap_or_else(|e| e.to_string());
    export_diagnostics(
        &PathBuf::from(destination),
        &[
            ("coexistence", report),
            (
                "database",
                format!(
                    "available={}",
                    state.database.lock().map_err(|e| e.to_string())?.is_some()
                ),
            ),
        ],
    )
    .map_err(text)
}

#[tauri::command]
fn autostart_status(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(text)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(text)
}

#[tauri::command]
async fn cloud_change_password(
    state: State<'_, ProductState>,
    current_password: String,
    new_password: String,
) -> Result<(), String> {
    let (base_url, access_token, vault_key) = cloud_commands::refresh_cloud_session(&state).await?;
    CloudClient::new(&base_url, is_local_development_url(&base_url))
        .map_err(text)?
        .change_password(
            &access_token,
            &current_password,
            &new_password,
            &vault_key,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .map_err(text)?;
    clear_cloud_refresh(&state)?;
    *state.cloud.lock().await = None;
    Ok(())
}

#[tauri::command]
async fn cloud_server_info(base_url: String) -> Result<serde_json::Value, String> {
    let development_http = url::Url::parse(&base_url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| matches!(host, "localhost" | "127.0.0.1"))
        })
        .unwrap_or(false);
    CloudClient::new(&base_url, development_http)
        .map_err(text)?
        .server_info(&tokio_util::sync::CancellationToken::new())
        .await
        .map_err(text)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudStatusView {
    configured: bool,
    logged_in: bool,
    base_url: Option<String>,
    account_name: Option<String>,
    device_id: Option<String>,
    pending_count: u64,
    failed_count: u64,
}

#[tauri::command]
async fn cloud_status(state: State<'_, ProductState>) -> Result<CloudStatusView, String> {
    let connection = node2socks_storage::open_and_migrate(database_path(&state)?).map_err(text)?;
    let profile: Option<(String, String, String)> = connection
        .query_row(
            "SELECT base_url,account_name,device_id FROM cloud_profiles WHERE is_active=1 LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(text)?;
    let pending_count = connection
        .query_row("SELECT count(*) FROM sync_outbox", [], |row| row.get(0))
        .map_err(text)?;
    let failed_count = connection
        .query_row(
            "SELECT count(*) FROM sync_outbox WHERE last_error IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(text)?;
    let logged_in = state.cloud.lock().await.is_some();
    Ok(CloudStatusView {
        configured: profile.is_some(),
        logged_in,
        base_url: profile.as_ref().map(|value| value.0.clone()),
        account_name: profile.as_ref().map(|value| value.1.clone()),
        device_id: profile.as_ref().map(|value| value.2.clone()),
        pending_count,
        failed_count,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CloudSessionView {
    device_id: String,
}

#[tauri::command]
async fn cloud_auth(
    state: State<'_, ProductState>,
    base_url: String,
    email: String,
    password: String,
    register: bool,
) -> Result<CloudSessionView, String> {
    let development_http = is_local_development_url(&base_url);
    let client = CloudClient::new(&base_url, development_http).map_err(text)?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let device_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into());
    let tokens = if register {
        client
            .register(email.trim(), &password, &device_name, &cancel)
            .await
    } else {
        client
            .login(email.trim(), &password, &device_name, &cancel)
            .await
    }
    .map_err(text)?;
    let vault_key = if register {
        let key = SecretKey::random();
        client
            .create_vault(&tokens.access_token, &password, &key, &cancel)
            .await
            .map_err(text)?;
        key
    } else {
        client
            .unlock_vault(&tokens.access_token, &password, &cancel)
            .await
            .map_err(text)?
    };
    let view = CloudSessionView {
        device_id: tokens.device_id.clone(),
    };
    let cursor = activate_cloud_profile(&state, &base_url, email.trim(), &tokens.device_id)?;
    persist_cloud_refresh(&state, &base_url, email.trim(), &tokens.refresh_token)?;
    *state.cloud.lock().await = Some(CloudSession {
        base_url,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        device_id: tokens.device_id,
        vault_key: vault_key.clone(),
        cursor,
    });
    let key_path = database_path(&state)?
        .parent()
        .ok_or_else(|| "无法定位数据目录".to_owned())?
        .join("cloud-vault-key.dpapi");
    let protected = protect_key(&vault_key).map_err(text)?;
    std::fs::write(&key_path, protected).map_err(text)?;
    *state.sync_key.lock().map_err(text)? = Some(vault_key.clone());
    Ok(view)
}

#[tauri::command]
async fn cloud_devices(state: State<'_, ProductState>) -> Result<Vec<CloudDevice>, String> {
    let (base_url, access_token, _) = cloud_commands::refresh_cloud_session(&state).await?;
    CloudClient::new(&base_url, is_local_development_url(&base_url))
        .map_err(text)?
        .devices(&access_token, &tokio_util::sync::CancellationToken::new())
        .await
        .map_err(text)
}

#[tauri::command]
async fn cloud_revoke_device(
    state: State<'_, ProductState>,
    device_id: String,
) -> Result<(), String> {
    let (base_url, access_token, _) = cloud_commands::refresh_cloud_session(&state).await?;
    if state
        .cloud
        .lock()
        .await
        .as_ref()
        .is_some_and(|session| session.device_id == device_id)
    {
        return Err("不能在此操作中吊销当前设备；请使用退出登录".into());
    }
    CloudClient::new(&base_url, is_local_development_url(&base_url))
        .map_err(text)?
        .revoke_device(
            &access_token,
            &device_id,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .map_err(text)
}

#[tauri::command]
async fn cloud_logout(
    state: State<'_, ProductState>,
) -> Result<crate::advanced_commands::MutationResult<()>, String> {
    let mut warning = None;
    if state.cloud.lock().await.is_some() {
        match cloud_commands::refresh_cloud_session(&state).await {
            Ok((base_url, access_token, _)) => {
                let remote = match CloudClient::new(&base_url, is_local_development_url(&base_url))
                {
                    Ok(client) => client
                        .logout(&access_token, &tokio_util::sync::CancellationToken::new())
                        .await
                        .map_err(text),
                    Err(error) => Err(text(error)),
                };
                if let Err(error) = remote {
                    warning = Some(format!("已在本机退出，但服务器注销失败：{error}"));
                }
            }
            Err(error) => warning = Some(format!("已在本机退出，但服务器会话刷新失败：{error}")),
        }
    }
    clear_cloud_refresh(&state)?;
    *state.cloud.lock().await = None;
    Ok(crate::advanced_commands::MutationResult { value: (), warning })
}

fn is_local_development_url(value: &str) -> bool {
    url::Url::parse(value)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| matches!(host, "localhost" | "127.0.0.1"))
        })
        .unwrap_or(false)
}

pub fn run() {
    tracing_subscriber::fmt().with_target(false).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--start-in-tray"]),
        ))
        .manage(ProductState {
            database: Mutex::new(None),
            master_key: Mutex::new(None),
            app_handle: Mutex::new(None),
            bridge: ProviderBridge::new(),
            bridge_handle: AsyncMutex::new(None),
            core: AsyncMutex::new(None),
            crash_monitor: AsyncMutex::new(None),
            core_running: AtomicBool::new(false),
            download_port: Mutex::new(None),
            cloud: AsyncMutex::new(None),
            sync_key: Mutex::new(None),
            refresh_cancel: CancellationToken::new(),
            refresh_task: AsyncMutex::new(None),
            health_results: AsyncMutex::new(HashMap::new()),
            node_latency_results: AsyncMutex::new(HashMap::new()),
            latency_jobs: AsyncMutex::new(HashMap::new()),
        })
        .setup(|app| {
            *app.state::<ProductState>()
                .app_handle
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))? = Some(app.handle().clone());
            let dir = match std::env::var_os("NODE2SOCKS_DATA_DIR") {
                Some(value) => {
                    let path = PathBuf::from(value);
                    if !path.is_absolute() {
                        return Err(std::io::Error::other(
                            "NODE2SOCKS_DATA_DIR must be an absolute path",
                        )
                        .into());
                    }
                    path
                }
                None => {
                    let executable_dir = std::env::current_exe()?.parent().map(PathBuf::from);
                    let portable_marker = executable_dir.as_ref().map(|path| path.join("portable.flag"));
                    if portable_marker.is_some_and(|path| path.is_file()) {
                        executable_dir.ok_or_else(|| std::io::Error::other("无法定位程序目录"))?.join("data")
                    } else {
                        app.path().app_local_data_dir()?
                    }
                },
            };
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("app.db");
            node2socks_storage::open_and_migrate(&path)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let key_path = dir.join("master-key.dpapi");
            let key = if key_path.exists() {
                unprotect_key(&std::fs::read(&key_path)?)
            } else {
                let key = SecretKey::random();
                let protected =
                    protect_key(&key).map_err(|e| std::io::Error::other(e.to_string()))?;
                std::fs::write(&key_path, protected)?;
                Ok(key)
            }
            .map_err(|e| std::io::Error::other(e.to_string()))?;
            let state = app.state::<ProductState>();
            *state
                .master_key
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))? = Some(key.clone());
            *state
                .database
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))? = Some(path.clone());
            let repository = SubscriptionRepository::new(
                node2socks_storage::open_and_migrate(&path)
                    .map_err(|e| std::io::Error::other(e.to_string()))?,
                key,
            );
            let sync_key_path = dir.join("cloud-vault-key.dpapi");
            if sync_key_path.exists() {
                let sync_key = unprotect_key(&std::fs::read(sync_key_path)?)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                *state
                    .sync_key
                    .lock()
                    .map_err(|e| std::io::Error::other(e.to_string()))? = Some(sync_key);
            }
            restore_cloud_session(&state).ok();
            let service = Arc::new(
                SubscriptionService::new(repository, state.bridge.clone(), 4)
                    .map_err(|e| std::io::Error::other(e.to_string()))?,
            );
            let handle = tauri::async_runtime::block_on(async {
                service.restore_bridge().await?;
                state.bridge.start().await
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;
            *tauri::async_runtime::block_on(state.bridge_handle.lock()) = Some(handle);
            let cancel = state.refresh_cancel.child_token();
            let app_handle = app.handle().clone();
            let refresh_task = tauri::async_runtime::spawn(async move {
                let period = std::time::Duration::from_secs(30);
                let mut interval = tokio::time::interval_at(
                    tokio::time::Instant::now() + period,
                    period,
                );
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = interval.tick() => {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|duration| duration.as_secs())
                                .unwrap_or(0);
                            if let Ok(ids) = service.due(now) {
                                for id in ids {
                                    let state = app_handle.state::<ProductState>();
                                    if let Err(error) = commands::refresh_subscription_inner(&state, id, &cancel).await {
                                        tracing::warn!(%error, %id, "automatic subscription refresh failed");
                                    }
                                    crate::events::emit_snapshot_dirty(&app_handle, "all");
                                }
                            }
                        }
                    }
                }
            });
            *tauri::async_runtime::block_on(state.refresh_task.lock()) = Some(refresh_task);
            if crate::advanced_commands::load_settings(&state)
                .map(|settings| settings.auto_start_core)
                .unwrap_or(false)
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let state = app_handle.state::<ProductState>();
                    if let Err(error) = commands::start_core_inner(&state).await {
                        tracing::warn!(%error, "automatic proxy core start failed");
                    } else {
                        crate::events::emit_snapshot_dirty(&app_handle, "dashboard");
                    }
                });
            }
            setup_tray(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            dashboard_snapshot,
            app_snapshot,
            create_backup,
            restore_backup,
            diagnostic_export,
            autostart_status,
            set_autostart,
            commands::create_subscription,
            commands::delete_subscription,
            crate::advanced_commands::get_subscription,
            crate::advanced_commands::save_subscription,
            crate::advanced_commands::update_subscription,
            crate::advanced_commands::set_subscription_enabled,
            crate::advanced_commands::refresh_all_subscriptions,
            cloud_server_info,
            cloud_status,
            commands::refresh_subscription,
            commands::list_nodes,
            crate::advanced_commands::list_node_views,
            crate::advanced_commands::start_latency_test,
            crate::advanced_commands::cancel_latency_test,
            crate::advanced_commands::test_node_latency,
            crate::advanced_commands::test_all_node_latencies,
            crate::advanced_commands::check_node,
            commands::create_slot,
            crate::advanced_commands::batch_create_slots,
            crate::advanced_commands::rename_slot,
            crate::advanced_commands::update_slot,
            crate::advanced_commands::suggest_slot_rebind,
            commands::delete_slot,
            crate::advanced_commands::batch_delete_slots,
            commands::bind_slot,
            commands::check_slot,
            commands::start_core,
            commands::stop_core,
            commands::list_network_adapters,
            commands::get_outbound_interface,
            commands::set_outbound_interface,
            crate::advanced_commands::get_settings,
            crate::advanced_commands::update_settings,
            crate::advanced_commands::data_directory,
            crate::advanced_commands::open_data_directory,
            crate::advanced_commands::open_github,
            cloud_auth,
            cloud_devices,
            cloud_revoke_device,
            cloud_change_password,
            cloud_logout,
            cloud_commands::cloud_push_local,
            cloud_commands::cloud_pull_merge
        ])
        .build(tauri::generate_context!())
        .expect("error while building Node2Socks")
        .run(|app, event| {
            if let RunEvent::Ready = event {
                let requested = std::env::args_os().any(|argument| argument == "--start-in-tray");
                let configured = crate::advanced_commands::load_settings(&app.state::<ProductState>())
                    .map(|settings| settings.start_in_tray)
                    .unwrap_or(false);
                if requested || configured {
                    enter_low_memory_tray_mode(app);
                    return;
                }
            }
            if let RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } = &event
                && label == "main"
            {
                api.prevent_close();
                enter_low_memory_tray_mode(app);
                return;
            }
            if let RunEvent::ExitRequested {
                code: None, api, ..
            } = &event
            {
                api.prevent_exit();
                return;
            }
            if let RunEvent::Exit = event {
                let state = app.state::<ProductState>();
                state.refresh_cancel.cancel();
                tauri::async_runtime::block_on(async {
                    if let Some(task) = state.refresh_task.lock().await.take() {
                        let _ = task.await;
                    }
                    if let Err(error) = commands::stop_core_inner(&state).await {
                        tracing::error!(%error, "failed to stop proxy core during exit");
                    }
                    if let Some(bridge) = state.bridge_handle.lock().await.take() {
                        bridge.shutdown().await;
                    }
                });
            }
        });
}

fn database_path(state: &ProductState) -> Result<PathBuf, String> {
    state
        .database
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "database not ready".into())
}
fn persist_cloud_refresh(
    state: &ProductState,
    base_url: &str,
    account_name: &str,
    refresh_token: &str,
) -> Result<(), String> {
    let key = state
        .master_key
        .lock()
        .map_err(text)?
        .clone()
        .ok_or("主密钥未初始化")?;
    let aad = format!(
        "cloud-refresh:{base_url}:{}",
        account_name.to_ascii_lowercase()
    );
    let cipher = encrypt(&key, refresh_token.as_bytes(), aad.as_bytes()).map_err(text)?;
    node2socks_storage::open_and_migrate(database_path(state)?).map_err(text)?.execute(
        "UPDATE cloud_profiles SET refresh_token_cipher=?1,updated_at=strftime('%s','now') WHERE base_url=?2 AND account_name=?3 AND is_active=1",
        params![cipher, base_url, account_name],
    ).map_err(text)?;
    Ok(())
}

fn clear_cloud_refresh(state: &ProductState) -> Result<(), String> {
    node2socks_storage::open_and_migrate(database_path(state)?)
        .map_err(text)?
        .execute(
            "UPDATE cloud_profiles SET refresh_token_cipher=NULL,updated_at=strftime('%s','now') WHERE is_active=1",
            [],
        )
        .map_err(text)?;
    Ok(())
}

fn restore_cloud_session(state: &ProductState) -> Result<(), String> {
    let connection = node2socks_storage::open_and_migrate(database_path(state)?).map_err(text)?;
    let saved: Option<(String, String, String, u64, Vec<u8>)> = connection.query_row(
        "SELECT base_url,account_name,device_id,last_cursor,refresh_token_cipher FROM cloud_profiles WHERE is_active=1 AND refresh_token_cipher IS NOT NULL LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).optional().map_err(text)?;
    let Some((base_url, account_name, device_id, cursor, cipher)) = saved else {
        return Ok(());
    };
    let key = state
        .master_key
        .lock()
        .map_err(text)?
        .clone()
        .ok_or("主密钥未初始化")?;
    let aad = format!(
        "cloud-refresh:{base_url}:{}",
        account_name.to_ascii_lowercase()
    );
    let refresh_token =
        String::from_utf8(decrypt(&key, &cipher, aad.as_bytes()).map_err(text)?).map_err(text)?;
    let vault_key = state
        .sync_key
        .lock()
        .map_err(text)?
        .clone()
        .ok_or("云 Vault 密钥不可用")?;
    *tauri::async_runtime::block_on(state.cloud.lock()) = Some(CloudSession {
        base_url,
        access_token: String::new(),
        refresh_token,
        device_id,
        vault_key,
        cursor,
    });
    Ok(())
}

fn activate_cloud_profile(
    state: &ProductState,
    base_url: &str,
    account_name: &str,
    device_id: &str,
) -> Result<u64, String> {
    let mut connection =
        node2socks_storage::open_and_migrate(database_path(state)?).map_err(text)?;
    let previous: Option<(String, String)> = connection
        .query_row(
            "SELECT base_url,account_name FROM cloud_profiles ORDER BY is_active DESC,updated_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(text)?;
    let switched_account = previous.is_some_and(|(previous_url, previous_account)| {
        previous_url != base_url || !previous_account.eq_ignore_ascii_case(account_name)
    });
    let existing: Option<(String, u64)> = connection
        .query_row(
            "SELECT id,last_cursor FROM cloud_profiles WHERE base_url=?1 AND account_name=?2 ORDER BY updated_at DESC LIMIT 1",
            params![base_url, account_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(text)?;
    let (id, saved_cursor) = existing.unwrap_or_else(|| (uuid::Uuid::new_v4().to_string(), 0));
    let cursor = if switched_account { 0 } else { saved_cursor };
    let transaction = connection.transaction().map_err(text)?;
    if switched_account {
        transaction
            .execute("DELETE FROM sync_outbox", [])
            .map_err(text)?;
        transaction
            .execute("DELETE FROM sync_versions", [])
            .map_err(text)?;
    }
    transaction
        .execute("UPDATE cloud_profiles SET is_active=0", [])
        .map_err(text)?;
    transaction
        .execute(
            "INSERT INTO cloud_profiles(id,base_url,account_name,device_id,is_active,last_cursor,created_at,updated_at) VALUES(?1,?2,?3,?4,1,?5,strftime('%s','now'),strftime('%s','now')) ON CONFLICT(id) DO UPDATE SET base_url=excluded.base_url,account_name=excluded.account_name,device_id=excluded.device_id,is_active=1,last_cursor=?5,updated_at=excluded.updated_at",
            params![id, base_url, account_name, device_id, cursor],
        )
        .map_err(text)?;
    transaction.commit().map_err(text)?;
    Ok(cursor)
}
fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
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
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let start_core = MenuItem::with_id(app, "start_core", "启动 Core", true, None::<&str>)?;
    let stop_core = MenuItem::with_id(app, "stop_core", "停止 Core", true, None::<&str>)?;
    let copy_all = MenuItem::with_id(
        app,
        "copy_all",
        "复制全部代理（SOCKS5 + 备注）",
        true,
        None::<&str>,
    )?;
    let open_data = MenuItem::with_id(app, "open_data", "打开数据目录", true, None::<&str>)?;
    let local_only = MenuItem::with_id(
        app,
        "local_only",
        "安全监听：127.0.0.1",
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "真正退出 Node2Socks", true, None::<&str>)?;
    let separator_one = PredefinedMenuItem::separator(app)?;
    let separator_two = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &separator_one,
            &start_core,
            &stop_core,
            &copy_all,
            &open_data,
            &separator_two,
            &local_only,
            &quit,
        ],
    )?;
    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Node2Socks · 本地代理槽位")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "start_core" => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<ProductState>();
                    if let Err(error) = commands::start_core_inner(&state).await {
                        tracing::error!(%error, "failed to start Core from tray");
                        show_main_window(&handle);
                    } else {
                        crate::events::emit_snapshot_dirty(&handle, "dashboard");
                    }
                });
            }
            "stop_core" => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<ProductState>();
                    if let Err(error) = commands::stop_core_inner(&state).await {
                        tracing::error!(%error, "failed to stop Core from tray");
                        show_main_window(&handle);
                    } else {
                        crate::events::emit_snapshot_dirty(&handle, "dashboard");
                    }
                });
            }
            "copy_all" => {
                let state = app.state::<ProductState>();
                if let Err(error) = copy_all_proxies_to_clipboard(&state) {
                    tracing::error!(%error, "failed to copy proxies from tray");
                    show_main_window(app);
                }
            }
            "open_data" => {
                if let Err(error) =
                    open_data_directory_from_tray(app.state::<ProductState>().inner())
                {
                    tracing::error!(%error, "failed to open data directory from tray");
                    show_main_window(app);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    tray = tray.icon(tray_icon);
    tray.build(app)?;
    Ok(())
}

fn copy_all_proxies_to_clipboard(state: &ProductState) -> Result<usize, String> {
    let mut slots = commands::slot_repository(state)?.list().map_err(text)?;
    slots.sort_by_key(|(slot, _)| slot.local_port);
    if slots.is_empty() {
        return Err("还没有可复制的代理槽位".into());
    }
    let content = slots
        .iter()
        .map(|(slot, _)| tray_proxy_line(&slot.name, slot.local_port))
        .collect::<Vec<_>>()
        .join("\r\n");
    #[cfg(windows)]
    set_windows_clipboard_text(&content)?;
    #[cfg(not(windows))]
    return Err("托盘复制仅支持 Windows".into());
    Ok(slots.len())
}

fn tray_proxy_line(name: &str, port: u16) -> String {
    let remark: String = name
        .chars()
        .map(|character| {
            if matches!(character, '{' | '}' | '\r' | '\n') {
                ' '
            } else {
                character
            }
        })
        .collect();
    format!("socks5://127.0.0.1:{port}{{{}}}", remark.trim())
}

#[cfg(windows)]
fn set_windows_clipboard_text(content: &str) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::GlobalFree,
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        },
    };
    let encoded: Vec<u16> = content.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err(format!(
                "无法打开 Windows 剪贴板：{}",
                std::io::Error::last_os_error()
            ));
        }
        struct ClipboardGuard;
        impl Drop for ClipboardGuard {
            fn drop(&mut self) {
                unsafe {
                    CloseClipboard();
                }
            }
        }
        let _guard = ClipboardGuard;
        if EmptyClipboard() == 0 {
            return Err(format!(
                "无法清空 Windows 剪贴板：{}",
                std::io::Error::last_os_error()
            ));
        }
        let memory = GlobalAlloc(GMEM_MOVEABLE, encoded.len() * std::mem::size_of::<u16>());
        if memory.is_null() {
            return Err("无法分配剪贴板内存".into());
        }
        let target = GlobalLock(memory).cast::<u16>();
        if target.is_null() {
            GlobalFree(memory);
            return Err("无法锁定剪贴板内存".into());
        }
        std::ptr::copy_nonoverlapping(encoded.as_ptr(), target, encoded.len());
        GlobalUnlock(memory);
        const CF_UNICODETEXT: u32 = 13;
        if SetClipboardData(CF_UNICODETEXT, memory).is_null() {
            GlobalFree(memory);
            return Err(format!(
                "无法写入 Windows 剪贴板：{}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

fn open_data_directory_from_tray(state: &ProductState) -> Result<(), String> {
    let path = commands::database_path(state)?
        .parent()
        .ok_or("无法定位数据目录")?
        .to_path_buf();
    #[cfg(windows)]
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map_err(text)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    let window = app.get_webview_window("main").or_else(|| {
        let config = app
            .config()
            .app
            .windows
            .iter()
            .find(|config| config.label == "main")
            .cloned()?;
        match WebviewWindowBuilder::from_config(app, &config).and_then(|builder| builder.build()) {
            Ok(window) => Some(window),
            Err(error) => {
                tracing::error!(%error, "failed to recreate main window from tray");
                None
            }
        }
    });
    if let Some(window) = window {
        let _ = window.set_skip_taskbar(false);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn enter_low_memory_tray_mode(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_skip_taskbar(true);
        if let Err(error) = window.destroy() {
            tracing::error!(%error, "failed to release hidden WebView memory");
        }
    }
    #[cfg(windows)]
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        trim_current_process_working_set();
    });
}

#[cfg(windows)]
fn trim_current_process_working_set() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

#[cfg(test)]
mod tray_tests {
    use super::tray_proxy_line;

    #[test]
    fn tray_proxy_export_preserves_chinese_and_sanitizes_remark_delimiters() {
        assert_eq!(
            tray_proxy_line(" 店铺{甲}\n", 21_001),
            "socks5://127.0.0.1:21001{店铺 甲}"
        );
    }
}
