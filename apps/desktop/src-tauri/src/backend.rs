use node2socks_crypto::{
    SecretKey,
    dpapi::{protect_key, unprotect_key},
};
use node2socks_diagnostics::{ClashMode, inspect_windows};
use node2socks_domain::SlotBindingState;
use node2socks_recovery::{backup_database, export_diagnostics};
use node2socks_slot_manager::{SlotRepository, SqliteSlotRepository};
use serde::Serialize;
use std::{path::PathBuf, sync::Mutex};
use tauri::{
    AppHandle, Manager, State,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};
pub struct ProductState {
    pub master_key: Mutex<Option<SecretKey>>,
    pub database: Mutex<Option<PathBuf>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Dashboard {
    version: &'static str,
    core_running: bool,
    local_mode_available: bool,
    slots: Vec<SlotSummary>,
    subscriptions: Vec<SubscriptionSummary>,
    diagnostics: DiagnosticSummary,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SlotSummary {
    id: String,
    name: String,
    port: u16,
    node_name: Option<String>,
    state: String,
    latency_ms: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubscriptionSummary {
    id: String,
    name: String,
    masked_url: String,
    node_count: u64,
    enabled: bool,
    last_status: String,
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

#[tauri::command]
fn dashboard_snapshot(state: State<'_, ProductState>) -> Result<Dashboard, String> {
    let path = state
        .database
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("database not ready")?;
    let connection = node2socks_storage::open_and_migrate(&path).map_err(|e| e.to_string())?;
    let slots = SqliteSlotRepository::new(connection)
        .list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(slot, binding)| SlotSummary {
            id: slot.id.to_string(),
            name: slot.name,
            port: slot.local_port,
            node_name: None,
            state: binding_state(binding.state).into(),
            latency_ms: None,
        })
        .collect();
    let report = inspect_windows().ok();
    let diagnostics = report
        .map(|value| DiagnosticSummary {
            core: "已停止".into(),
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
            core: "已停止".into(),
            clash: "未检测".into(),
            system_proxy: "未启用".into(),
            tun: "未检测".into(),
            outbound_adapter: "跟随系统路由".into(),
            warning: None,
        });
    Ok(Dashboard {
        version: env!("CARGO_PKG_VERSION"),
        core_running: false,
        local_mode_available: true,
        slots,
        subscriptions: Vec::new(),
        diagnostics,
    })
}
#[tauri::command]
fn create_backup(state: State<'_, ProductState>, destination: String) -> Result<String, String> {
    let path = state
        .database
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("database not ready")?;
    backup_database(&path, &PathBuf::from(destination))
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn diagnostic_export(state: State<'_, ProductState>, destination: String) -> Result<(), String> {
    let db = state.database.lock().map_err(|e| e.to_string())?.clone();
    let report = inspect_windows()
        .map(|v| format!("{v:#?}"))
        .unwrap_or_else(|e| e.to_string());
    export_diagnostics(
        &PathBuf::from(destination),
        &[
            ("coexistence", report),
            ("database", format!("available={}", db.is_some())),
        ],
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn autostart_status(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|e| e.to_string())
}

pub fn run() {
    tracing_subscriber::fmt().with_target(false).init();
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(ProductState {
            database: Mutex::new(None),
            master_key: Mutex::new(None),
        })
        .setup(|app| {
            let dir = app.path().app_local_data_dir()?;
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
            *app.state::<ProductState>()
                .master_key
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))? = Some(key);
            *app.state::<ProductState>()
                .database
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))? = Some(path);
            setup_tray(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            dashboard_snapshot,
            create_backup,
            diagnostic_export,
            autostart_status,
            set_autostart
        ])
        .run(tauri::generate_context!())
        .expect("error while running Node2Socks")
}
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开 Node2Socks", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Node2Socks · 本地代理槽位")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
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
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;
    Ok(())
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
#[allow(dead_code)]
fn clash_label(value: ClashMode) -> &'static str {
    match value {
        ClashMode::NotDetected => "未检测",
        ClashMode::SystemProxy => "System Proxy",
        ClashMode::TunSuspected => "疑似 TUN",
        ClashMode::ProcessOnly => "仅进程",
    }
}
