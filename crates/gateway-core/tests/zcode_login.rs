//! T4.1c zcode CLI 设备码登录 + 余额（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md zcode 节——
//! `POST /api/v1/oauth/cli/init`（Bearer 自生成 32 字节 hex，body {provider:"bigmodel"}）
//! → `GET /api/v1/oauth/cli/poll/{flow_id}`（pending 继续轮询）→ 凭据；
//! 余额 `GET /api/v1/zcode-plan/billing/balance` 需 Authorization + X-Device-Mid
//! （缺分别为 401 / 400 code 3001）。

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::providers::zcode::{ZcodeBalance, ZcodeProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Stub {
    init_auth: Option<String>,
    init_body: Option<Value>,
    poll_hits: usize,
    balance_headers: Option<HeaderMap>,
}

async fn init(
    State(st): State<Arc<Mutex<Stub>>>,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> impl IntoResponse {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    let auth = headers.get("authorization").and_then(|x| x.to_str().ok()).unwrap_or("").to_string();
    *st.lock().unwrap() = Stub {
        init_auth: Some(auth),
        init_body: Some(v),
        ..Default::default()
    };
    Json(json!({ "flow_id": "flow-abc", "url": "https://zcode.z.ai/oauth/authorize?flow=flow-abc" }))
}

async fn poll(State(st): State<Arc<Mutex<Stub>>>, Path(flow): Path<String>) -> impl IntoResponse {
    assert_eq!(flow, "flow-abc");
    let hits = {
        let mut s = st.lock().unwrap();
        s.poll_hits += 1;
        s.poll_hits
    };
    if hits < 3 {
        return Json(json!({ "status": "pending" }));
    }
    Json(json!({ "status": "ok", "zcodejwttoken": "jwt-final-token" }))
}

async fn balance(
    State(st): State<Arc<Mutex<Stub>>>,
    headers: HeaderMap,
) -> axum::response::Response {
    *st.lock().unwrap().balance_headers.get_or_insert_with(HeaderMap::new) = headers.clone();
    if !headers.contains_key("authorization") {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response();
    }
    if !headers.contains_key("x-device-mid") {
        return (StatusCode::BAD_REQUEST, Json(json!({ "code": 3001 }))).into_response();
    }
    Json(json!({ "data": { "total": 100_000_000, "used": 37_500_000 } })).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Stub>>) {
    let st = Arc::new(Mutex::new(Stub::default()));
    let app = Router::new()
        .route("/api/v1/oauth/cli/init", post(init))
        .route("/api/v1/oauth/cli/poll/{flow_id}", get(poll))
        .route("/api/v1/zcode-plan/billing/balance", get(balance))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), st)
}

#[tokio::test]
async fn login_init_sends_hex_bearer_and_bigmodel_provider() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let flow = pv.login_init().await.unwrap();
    assert_eq!(flow.flow_id, "flow-abc");
    assert!(flow.auth_url.contains("zcode.z.ai"));
    let s = st.lock().unwrap();
    let auth = s.init_auth.as_deref().unwrap();
    assert!(auth.starts_with("Bearer "), "init 需带 Bearer：{auth}");
    let hex = auth.trim_start_matches("Bearer ");
    assert_eq!(hex.len(), 64, "自生成 32 字节 hex：{hex}");
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(s.init_body.as_ref().unwrap()["provider"], json!("bigmodel"));
}

#[tokio::test]
async fn login_polls_until_ok_and_returns_credential() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let cred = pv.login_with_interval(std::time::Duration::from_millis(5)).await.unwrap();
    assert_eq!(cred.secret, "jwt-final-token");
    assert_eq!(st.lock().unwrap().poll_hits, 3, "pending 两次 + ok 一次");
}

#[tokio::test]
async fn balance_sends_identity_and_parses_tokens() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential { account_id: "a".into(), secret: "jwt".into() };
    let b: ZcodeBalance = pv.balance(&cred).await.unwrap();
    assert_eq!(b.total_tokens, 100_000_000);
    assert_eq!(b.used_tokens, 37_500_000);
    let headers = st.lock().unwrap().balance_headers.clone().unwrap();
    assert!(headers.contains_key("x-device-mid"), "缺 X-Device-Mid 上游 400/3001");
}
