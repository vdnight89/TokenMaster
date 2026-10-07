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
    ledger: std::sync::Arc<gateway_core::ledger::Ledger>,
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
        .manage(GatewayState {
            handle: Mutex::new(None),
            ledger: std::sync::Arc::new(gateway_core::ledger::Ledger::memory()),
        })
        .manage(CaptchaState { pool: std::sync::Arc::new(gateway_core::captcha_carrier::CaptchaSupplyPool::new()) })
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
            let handle = tauri::async_runtime::block_on(gateway_core::server::start_with_ledger(
                config,
                Vec::new(),
                Default::default(),
                Some(state.ledger.clone()),
            ))
            .map_err(|e| format!("gateway start failed: {e}"))?;
            // 锁中毒不致命（Option 槽无 invariant）：恢复数据继续
            *state.handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);

            // T6.1 托盘：菜单（显示/退出）+ tooltip
            let _tray = app.tray_by_id("main");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
                ping,
                gateway_status,
                list_providers,
                list_accounts,
                gateway_config,
                usage_summary,
                open_captcha_carrier,
                captcha_param_ready,
                captcha_carrier_failed,
                list_logs,
            ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ───────────── T5.11 IPC Commands ─────────────

#[tauri::command]
fn list_providers() -> Result<serde_json::Value, String> {
    let providers = [
        ("zcode", "ZCode", "#3b82f6", "PRO"),
        ("gemini", "Gemini", "#10b981", "ULTRA"),
        ("trae", "Trae", "#f97316", "PRO"),
        ("qoder", "Qoder", "#8b5cf6", "PRO"),
        ("qodercn", "QoderCN", "#a855f7", "PRO"),
        ("minimax", "MiniMax", "#ec4899", "PRO"),
        ("codearts", "CodeArts", "#0ea5e9", "PRO"),
        ("buddy", "Buddy", "#14b8a6", "PRO"),
        ("workbuddy", "WorkBuddy", "#84cc16", "FREE"),
        ("lobsterai", "LobsterAI", "#ef4444", "PRO"),
        ("cline", "Cline", "#06b6d4", "FREE"),
        ("loomy", "Loomy", "#6366f1", "FREE"),
        ("raccoon", "Raccoon", "#d946ef", "PRO"),
        ("opencode", "OpenCode", "#64748b", "FREE"),
        ("commandcode", "CommandCode", "#eab308", "PRO"),
    ];
    let mut map = serde_json::Map::new();
    for (id, name, color, tier) in providers {
        map.insert(id.to_string(), serde_json::json!({
            "name": name, "color": color, "tier": tier
        }));
    }
    Ok(serde_json::Value::Object(map))
}

#[tauri::command]
fn list_accounts() -> Result<serde_json::Value, String> {
    let store = gateway_core::store::Store::open_default()
        .map_err(|e| format!("store open failed: {e}"))?;
    let accounts = store
        .load_accounts()
        .map_err(|e| format!("load accounts failed: {e}"))?;
    let items: Vec<serde_json::Value> = accounts
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "provider": a.provider,
                "name": a.label,
                "state": if a.enabled { "ok" } else { "off" },
                "created_at": a.created_at,
                "updated_at": a.updated_at,
            })
        })
        .collect();
    Ok(serde_json::Value::Array(items))
}

#[tauri::command]
fn gateway_config() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "port": GATEWAY_PORT,
        "base_url": format!("http://127.0.0.1:{GATEWAY_PORT}"),
        "protocol": "OpenAI + Anthropic",
    }))
}

#[tauri::command]
fn usage_summary(state: State<GatewayState>) -> Result<serde_json::Value, String> {
    let ledger = &state.ledger;
    let today_start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400 * 86400;
    let recent = ledger.recent(500);
    let today: Vec<_> = recent.iter().filter(|r| r.ts >= today_start).collect();
    let today_requests = today.len() as u64;
    // token 数来自上游响应，饱和累加防巨数溢出 panic
    let today_tokens: u64 = today
        .iter()
        .map(|r| r.prompt_tokens.saturating_add(r.completion_tokens))
        .fold(0u64, u64::saturating_add);
    let mut by_provider = serde_json::Map::new();
    for b in ledger.aggregate_by_provider() {
        by_provider.insert(b.key.clone(), serde_json::json!({
            "requests": b.requests, "prompt": b.prompt_tokens, "completion": b.completion_tokens, "failures": b.failures
        }));
    }
    let mut by_model = serde_json::Map::new();
    for b in ledger.aggregate_by_model() {
        by_model.insert(b.key.clone(), serde_json::json!({
            "requests": b.requests, "prompt": b.prompt_tokens, "completion": b.completion_tokens, "failures": b.failures
        }));
    }
    Ok(serde_json::json!({
        "today": { "requests": today_requests, "tokens": today_tokens },
        "byProvider": by_provider,
        "byModel": by_model,
    }))
}

#[tauri::command]
fn list_logs(state: State<GatewayState>, limit: Option<usize>) -> Result<serde_json::Value, String> {
    let n = limit.unwrap_or(50).min(200);
    let recent = state.ledger.recent(n);
    let items: Vec<serde_json::Value> = recent
        .iter()
        .rev()
        .map(|r| {
            serde_json::json!({
                "ts": r.ts,
                "provider": r.provider,
                "account": r.account_id,
                "model": r.model,
                "prompt_tokens": r.prompt_tokens,
                "completion_tokens": r.completion_tokens,
                "status": r.status,
                "ttfb_ms": r.ttfb_ms,
                "duration_ms": r.duration_ms,
                "proto": r.proto,
            })
        })
        .collect();
    Ok(serde_json::Value::Array(items))
}

// ───────────── T6.3 验证码载体：WebView 子窗口 ─────────────

use std::sync::Arc;

struct CaptchaState {
    pool: Arc<gateway_core::captcha_carrier::CaptchaSupplyPool>,
}

#[tauri::command]
fn open_captcha_carrier(app: tauri::AppHandle, _state: State<CaptchaState>) -> Result<(), String> {
    use tauri::WebviewWindowBuilder;
    // 已有载体窗口则聚焦
    if let Some(win) = app.get_webview_window("captcha-carrier") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    // 创建隐藏载体窗口（加载 zcode.z.ai origin 让 AliyunCaptcha SDK 在真实 origin 下运行）
    let win = WebviewWindowBuilder::new(
        &app,
        "captcha-carrier",
        tauri::WebviewUrl::External("https://zcode.z.ai/".parse::<tauri::Url>().map_err(|e| e.to_string())?),
    )
    .title("TokenMaster 验证码")
    .inner_size(420.0, 320.0)
    .visible(true) // 用户需要看到并操作验证码
    .resizable(false)
    .decorations(true)
    .build()
    .map_err(|e| format!("创建载体窗口失败：{e}"))?;
    let _ = win;
    Ok(())
}

#[tauri::command]
fn captcha_param_ready(state: State<CaptchaState>, param: String, region: String) -> Result<(), String> {
    state.pool.push_param(&param, &region);
    Ok(())
}

#[tauri::command]
fn captcha_carrier_failed(state: State<CaptchaState>) -> Result<serde_json::Value, String> {
    let backoff = state.pool.record_carrier_failure();
    Ok(serde_json::json!({ "backoff_secs": backoff.as_secs() }))
}
