//! Tauri 壳：窗口 + IPC 命令 + 内嵌网关生命周期。
//! 网关核心是纯库（gateway-core），这里只做接线（ADR-0004 的壳层约束）。

use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::key::generate_gateway_key;
use gateway_core::server::GatewayHandle;
use std::sync::Mutex;
use tauri::{Manager, State};

/// 默认网关端口：与 round-02 原型一致（GUI 网关页可改，接 IPC 后生效）。
pub const GATEWAY_PORT: u16 = 8787;

struct GatewayState {
    handle: Mutex<Option<GatewayHandle>>,
}

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
fn gateway_status(state: State<GatewayState>) -> Result<serde_json::Value, String> {
    let guard = state.handle.lock().map_err(|e| e.to_string())?;
    match guard.as_ref() {
        Some(h) => Ok(serde_json::json!({ "running": true, "addr": h.addr.to_string() })),
        None => Ok(serde_json::json!({ "running": false })),
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 单实例：已有实例运行时聚焦主窗口
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(GatewayState { handle: Mutex::new(None) })
        .on_window_event(|window, event| {
            // 关窗最小化到托盘（不退出）；托盘菜单的「退出」才真正 close
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .setup(|app| {
            let state = app.state::<GatewayState>();
            // 网关随 GUI 启动（spec：桌面内嵌运行）。密钥先随机生成；
            // 持久化与 GUI 管理在 T5.11/M6 接入 Store 后完善。
            let config = GatewayConfig {
                port: Some(GATEWAY_PORT),
                auth: AuthMode::Required(generate_gateway_key()),
                model_map: Vec::new(),
                registry: Default::default(),
            };
            let handle = tauri::async_runtime::block_on(gateway_core::server::start_full(
                config,
                Vec::new(),
                Default::default(),
            ))
            .map_err(|e| format!("gateway start failed: {e}"))?;
            *state.handle.lock().unwrap() = Some(handle);

            // T6.1 托盘：菜单（显示/退出）+ tooltip
            let _tray = app.tray_by_id("main");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![ping, gateway_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
