//! T4.1c zcode CLI 设备码登录 + 余额（缝 2：stub 上游）。
//! 行为来源：zcode-login.ts（官方 startOAuthWithPolling 解出）——
//! `POST /api/v1/oauth/cli/init`（Bearer 自生成 32 字节 hex，body {provider:"bigmodel"}）
//! → 响应字段在 `data` 下（flow_id/authorize_url/expires_at/poll_interval_sec）；
//! `GET /api/v1/oauth/cli/poll/{flow_id}`（`data.status` pending/ready/failed；
//! 4xx（除 408/429）终态失败，网络/5xx 继续轮询）；
//! 余额 `GET /api/v1/zcode-plan/billing/balance` 需 Authorization + X-Device-Mid
//! （缺分别为 401 / 400 code 3001），并合并 preview 可领摘要（每日赠送
//! 不在 balances 里——upstream.ts:326-382）。

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::providers::zcode::ZcodeProvider;
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
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    let auth = headers
        .get("authorization")
        .and_then(|x| x.to_str().ok())
        .unwrap_or("")
        .to_string();
    *st.lock().unwrap() = Stub {
        init_auth: Some(auth),
        init_body: Some(v),
        ..Default::default()
    };
    // 官方 wire 形状：字段在 data 下（zcode-login.ts:198-241）
    Json(json!({
        "code": 0, "msg": "ok",
        "data": {
            "flow_id": "flow-abc",
            "poll_token": "ptok",
            "authorize_url": "https://zcode.z.ai/oauth/authorize?flow=flow-abc",
            "expires_at": 4102444800u64,
            "poll_interval_sec": 2
        }
    }))
}

async fn poll(State(st): State<Arc<Mutex<Stub>>>, Path(flow): Path<String>) -> impl IntoResponse {
    assert_eq!(flow, "flow-abc");
    let hits = {
        let mut s = st.lock().unwrap();
        s.poll_hits += 1;
        s.poll_hits
    };
    if hits < 3 {
        return Json(json!({ "code": 0, "data": { "status": "pending" } }));
    }
    Json(json!({
        "code": 0,
        "data": {
            "status": "ready",
            "token": "jwt-final-token",
            "user": { "user_id": "u-123", "name": "tester" }
        }
    }))
}

async fn balance(
    State(st): State<Arc<Mutex<Stub>>>,
    headers: HeaderMap,
) -> axum::response::Response {
    *st.lock()
        .unwrap()
        .balance_headers
        .get_or_insert_with(HeaderMap::new) = headers.clone();
    if !headers.contains_key("authorization") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    if !headers.contains_key("x-device-mid") {
        return (StatusCode::BAD_REQUEST, Json(json!({ "code": 3001 }))).into_response();
    }
    // 参考实测形状（upstream.ts:256-330）：data.balances[] 桶
    Json(json!({
        "code": 0,
        "data": {
            "displayMode": "personal",
            "balances": [
                {
                    "plan_id": "start-plan-trust-1003",
                    "show_name": "ZCode 免费额度",
                    "unit_type": "token", "meter": "model_usage",
                    "total_units": 100_000_000, "used_units": 37_500_000,
                    "remaining_units": 62_500_000, "available_units": 62_500_000,
                    "expires_at": 1893456000
                },
                {
                    "plan_id": "pack-extra",
                    "unit_type": "token",
                    "total_units": 10_000_000, "used_units": 0,
                    "remaining_units": 10_000_000
                }
            ]
        }
    }))
    .into_response()
}

async fn preview() -> impl IntoResponse {
    // 每日赠送只在 preview.plans（balances 读不到）——balance 需合并摘要
    Json(json!({
        "code": 0,
        "data": { "plans": [
            { "plan_id": "start-plan-trust-1003", "name": "每日赠送", "priority": 5,
              "entitlements": [ { "meter": "model_usage", "unit_type": "token",
                                   "grant_units": 100_000_000 } ] }
        ] }
    }))
}

async fn spawn() -> (String, Arc<Mutex<Stub>>) {
    let st = Arc::new(Mutex::new(Stub::default()));
    let app = Router::new()
        .route("/api/v1/oauth/cli/init", post(init))
        .route("/api/v1/oauth/cli/poll/{flow_id}", get(poll))
        .route("/api/v1/zcode-plan/billing/balance", get(balance))
        .route("/api/v1/zcode-plan/billing/preview", get(preview))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
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
async fn login_polls_until_ready_and_returns_credential() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let cred = pv
        .login_with_interval(std::time::Duration::from_millis(5))
        .await
        .unwrap();
    assert_eq!(cred.secret, "jwt-final-token");
    // user_id 是唯一稳定账号标识（zcode-auth.ts:550-559）
    assert_eq!(cred.account_id, "u-123");
    assert_eq!(st.lock().unwrap().poll_hits, 3, "pending 两次 + ready 一次");
}

#[tokio::test]
async fn login_poll_terminal_4xx_fails() {
    // 4xx（除 408/429）是终态失败；5xx/网络抖动继续轮询（zcode-login.ts:276-284）
    let app = Router::new()
        .route(
            "/api/v1/oauth/cli/init",
            post(|| async {
                Json(json!({ "code": 0, "data": {
                "flow_id": "flow-x", "authorize_url": "https://zcode.z.ai/a",
                "expires_at": 4102444800u64, "poll_interval_sec": 2 } }))
            }),
        )
        .route(
            "/api/v1/oauth/cli/poll/{flow_id}",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "code": 3004, "msg": "expired" })),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let pv = ZcodeProvider::new(format!("http://{addr}"));
    let err = pv
        .login_with_interval(std::time::Duration::from_millis(5))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("403"), "终态 4xx 原样上抛：{err}");
}

#[tokio::test]
async fn balance_sends_identity_parses_tokens_and_merges_claimable() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential {
        account_id: "a".into(),
        secret: "jwt".into(),
    };
    let b = pv.balance(&cred).await.unwrap();
    assert_eq!(b.remaining, 72_500_000, "Σ(available)：62.5M + 10M");
    assert_eq!(b.total, 110_000_000);
    assert_eq!(b.expires_at, Some(1893456000), "最早到期");
    assert_eq!(b.buckets.len(), 2);
    assert_eq!(
        b.buckets[0].unit_type.as_deref(),
        Some("token"),
        "单位是 token 不是积分"
    );
    assert!(!b.enterprise);
    // 每日赠送合并（balances 空、preview 有 ⇒ 不能显示 0）
    assert_eq!(b.claimable_plans.len(), 1);
    assert_eq!(b.claimable_plans[0].plan_id, "start-plan-trust-1003");
    assert_eq!(
        b.claimable_plans[0].tokens, 100_000_000,
        "量取自 entitlements[].grant_units"
    );
    let headers = st.lock().unwrap().balance_headers.clone().unwrap();
    assert!(
        headers.contains_key("x-device-mid"),
        "缺 X-Device-Mid 上游 400/3001"
    );
}
