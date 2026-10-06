//! T4.1d zcode 套餐领取链（缝 2：stub 上游）。
//! 行为来源：reference/deepseek-harness-codearts.md + reference/zcode-pool.md——
//! 激活事件上报（app_launch/app_daily_active/app_login_success，不补则 preview 恒空）
//! → `GET /api/v1/zcode-plan/billing/preview?app_version&platform`（带 mid 头）
//! → `POST /billing/claim {plan_id}`（验证码头先探后取；1003 幂等成功；1005 带 next_at 冷却）。

use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::providers::zcode::{ClaimOutcome, ZcodeProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default, Clone)]
struct Stub {
    events: Vec<String>,
    preview_query: Option<String>,
    claim_bodies: Vec<Value>,
    claim_headers: Vec<HeaderMap>,
}

async fn report(State(st): State<Arc<Mutex<Stub>>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let mut names = Vec::new();
    if let Some(arr) = v.get("events").and_then(Value::as_array) {
        for e in arr {
            if let Some(n) = e.get("name").and_then(Value::as_str) {
                names.push(n.to_string());
            }
        }
    }
    st.lock().unwrap().events = names;
    Json(json!({ "code": 0 })).into_response()
}

async fn preview(State(st): State<Arc<Mutex<Stub>>>, Query(q): Query<Vec<(String, String)>>) -> Response {
    st.lock().unwrap().preview_query =
        Some(q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
    Json(json!({
        "code": 0,
        "data": { "plans": [
            { "plan_id": "plan-glm-pro-14d", "title": "GLM Coding Pro · 14 天",
              "entitlements": [ { "meter": "model_usage", "unit_type": "token", "amount": 100_000_000 } ] }
        ] }
    }))
    .into_response()
}

async fn claim(State(st): State<Arc<Mutex<Stub>>>, headers: HeaderMap, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    {
        let mut s = st.lock().unwrap();
        s.claim_bodies.push(v.clone());
        s.claim_headers.push(headers.clone());
    }
    let has_captcha = headers.contains_key("x-aliyun-captcha-verify-param");
    if !has_captcha {
        return (StatusCode::OK, Json(json!({ "code": 3007, "message": "captcha required" }))).into_response();
    }
    // 凭 plan_id 决定结果：p1003 已领取、p1005 冷却、其余成功
    let pid = v.get("plan_id").and_then(Value::as_str).unwrap_or("");
    match pid {
        "p1003" => (StatusCode::OK, Json(json!({ "code": 1003, "message": "already claimed" }))).into_response(),
        "p1005" => (
            StatusCode::OK,
            Json(json!({ "code": 1005, "data": { "plan": { "ends_at": "2026-10-08T00:00:00Z" } } })),
        )
            .into_response(),
        _ => (StatusCode::OK, Json(json!({ "code": 0, "data": { "plan": { "ends_at": "2026-10-21T00:00:00Z" } } }))).into_response(),
    }
}

async fn spawn() -> (String, Arc<Mutex<Stub>>) {
    let st = Arc::new(Mutex::new(Stub::default()));
    let app = Router::new()
        .route("/api/v1/event/report", post(report))
        .route("/api/v1/zcode-plan/billing/preview", get(preview))
        .route("/api/v1/zcode-plan/billing/claim", post(claim))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), st)
}

fn cred() -> Credential {
    Credential { account_id: "zc-user-1".into(), secret: "jwt".into() }
}

#[tokio::test]
async fn report_activity_sends_three_activation_events() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    pv.report_activity(&cred()).await.unwrap();
    let names = st.lock().unwrap().events.clone();
    for expected in ["app_launch", "app_daily_active", "app_login_success"] {
        assert!(names.iter().any(|n| n == expected), "缺激活事件 {expected}: {names:?}");
    }
}

#[tokio::test]
async fn preview_lists_plans_with_token_entitlements() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let plans = pv.claim_preview(&cred()).await.unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].plan_id, "plan-glm-pro-14d");
    assert_eq!(plans[0].title, "GLM Coding Pro · 14 天");
    assert_eq!(plans[0].tokens, 100_000_000);
    let q = st.lock().unwrap().preview_query.clone().unwrap();
    assert!(q.contains("app_version") && q.contains("platform"), "preview 需带 query：{q}");
}

#[tokio::test]
async fn claim_without_captcha_returns_need_captcha() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let out = pv.submit_claim(&cred(), "plan-glm-pro-14d", None).await.unwrap();
    assert!(matches!(out, ClaimOutcome::NeedCaptcha), "先探后取：{out:?}");
    // 第二次带头 → 成功
    let cap = gateway_core::providers::zcode::CaptchaParam {
        param: "x".repeat(220),
        region: "cn-hangzhou".into(),
    };
    let out2 = pv.submit_claim(&cred(), "plan-glm-pro-14d", Some(&cap)).await.unwrap();
    assert!(matches!(out2, ClaimOutcome::Claimed { .. }), "{out2:?}");
    let headers = st.lock().unwrap().claim_headers[1].clone();
    assert!(headers.contains_key("x-aliyun-captcha-verify-param"));
    assert!(headers.contains_key("x-aliyun-captcha-verify-region"));
}

#[tokio::test]
async fn claim_1003_is_idempotent_success_and_1005_is_cooldown() {
    let (base, _) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let cap = gateway_core::providers::zcode::CaptchaParam {
        param: "x".repeat(220),
        region: "cn-hangzhou".into(),
    };
    let out = pv.submit_claim(&cred(), "p1003", Some(&cap)).await.unwrap();
    assert!(matches!(out, ClaimOutcome::AlreadyClaimed));
    let out2 = pv.submit_claim(&cred(), "p1005", Some(&cap)).await.unwrap();
    match out2 {
        ClaimOutcome::Cooldown { next_at } => assert!(next_at.contains("2026-10-08")),
        other => panic!("expect Cooldown, got {other:?}"),
    }
}

#[tokio::test]
async fn claim_daily_orchestrates_report_preview_claim() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let out = pv.claim_daily(&cred()).await.unwrap();
    assert!(matches!(out, ClaimOutcome::NeedCaptcha), "首轮无验证码应走到先探：{out:?}");
    assert!(!st.lock().unwrap().events.is_empty(), "claim_daily 必须先补激活事件");
    assert_eq!(st.lock().unwrap().claim_bodies.len(), 1);
}
