//! T4.1d zcode 套餐领取链（缝 2：stub 上游）。
//! 行为来源：zcode-auth.ts claimDailyWith + zcode-upstream.ts——
//! 激活事件上报（app_launch/app_daily_active，**各一条独立 POST**，body
//! {event, device_mid, platform, app_version}；不补则 preview 恒空；失败不阻塞）
//! → `GET /api/v1/zcode-plan/billing/preview?app_version&platform`（带 mid 头；
//! 量取 entitlements[].grant_units；priority 降序）
//! → `POST /billing/claim {plan_id}`（验证码头先探后取；本地深校验；
//! 1003 幂等成功；1005 冷却带 next_at；3007 换 param；3012 风控冷却）。

use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::providers::zcode::{CaptchaParam, ClaimOutcome, ZcodeProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default, Clone)]
struct Stub {
    events: Vec<String>,
    event_bodies: Vec<Value>,
    preview_query: Option<String>,
    claim_bodies: Vec<Value>,
    claim_headers: Vec<HeaderMap>,
}

async fn report(State(st): State<Arc<Mutex<Stub>>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let mut names = Vec::new();
    if let Some(event) = v.get("event").and_then(Value::as_str) {
        names.push(event.to_string());
    }
    {
        let mut s = st.lock().unwrap();
        s.events.extend(names);
        s.event_bodies.push(v);
    }
    Json(json!({ "code": 0 })).into_response()
}

async fn preview(
    State(st): State<Arc<Mutex<Stub>>>,
    Query(q): Query<Vec<(String, String)>>,
) -> Response {
    st.lock().unwrap().preview_query = Some(
        q.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&"),
    );
    // 参考形状（upstream.ts:416-460）：plan 顶层 name/priority，量在
    // entitlements[].grant_units（不是顶层 amount）；priority 降序领取。
    Json(json!({
        "code": 0,
        "data": { "plans": [
            { "plan_id": "plan-low", "name": "小额档", "priority": 1,
              "entitlements": [ { "meter": "model_usage", "unit_type": "token", "grant_units": 10_000_000 } ] },
            { "plan_id": "plan-glm-pro-14d", "name": "GLM Coding Pro · 14 天", "priority": 9,
              "entitlements": [ { "meter": "model_usage", "unit_type": "token", "grant_units": 100_000_000 } ] }
        ] }
    }))
    .into_response()
}

async fn claim(
    State(st): State<Arc<Mutex<Stub>>>,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    {
        let mut s = st.lock().unwrap();
        s.claim_bodies.push(v.clone());
        s.claim_headers.push(headers.clone());
    }
    let has_captcha = headers.contains_key("x-aliyun-captcha-verify-param");
    if !has_captcha {
        return (
            StatusCode::OK,
            Json(json!({ "code": 3007, "message": "captcha required" })),
        )
            .into_response();
    }
    // 凭 plan_id 决定结果：p1003 已领取、p1005 冷却、p3012 风控、其余成功
    let pid = v.get("plan_id").and_then(Value::as_str).unwrap_or("");
    match pid {
        "p1003" => (
            StatusCode::OK,
            Json(json!({ "code": 1003, "message": "already claimed" })),
        )
            .into_response(),
        "p1005" => (
            StatusCode::OK,
            Json(
                json!({ "code": 1005, "data": { "plan": { "ends_at": "2026-10-08T00:00:00Z" } } }),
            ),
        )
            .into_response(),
        "p3012" => (
            StatusCode::OK,
            Json(
                json!({ "code": 3012, "msg": "request has been blocked due to unusual activity." }),
            ),
        )
            .into_response(),
        _ => (
            StatusCode::OK,
            Json(json!({ "code": 0, "data": { "plan": { "ends_at": "2026-10-21T00:00:00Z" } } })),
        )
            .into_response(),
    }
}

async fn spawn() -> (String, Arc<Mutex<Stub>>) {
    let st = Arc::new(Mutex::new(Stub::default()));
    let app = Router::new()
        .route("/api/v1/event/report", post(report))
        .route("/api/v1/zcode-plan/billing/preview", get(preview))
        .route("/api/v1/zcode-plan/billing/claim", post(claim))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), st)
}

fn cred() -> Credential {
    Credential {
        account_id: "zc-user-1".into(),
        secret: "jwt".into(),
    }
}

/// 构造本地校验能过的 captcha param（长度 ≥200 + base64 JSON +
/// certifyId + securityToken ≥50——zcode-captcha.ts:163-200）。
fn valid_captcha() -> CaptchaParam {
    use base64::Engine as _;
    let payload = format!(
        r#"{{"certifyId":"{}","securityToken":"{}"}}"#,
        "c".repeat(80),
        "t".repeat(80)
    );
    CaptchaParam {
        param: base64::engine::general_purpose::STANDARD.encode(payload),
        region: "cn-hangzhou".into(),
    }
}

#[tokio::test]
async fn report_activity_sends_two_activation_events_as_separate_posts() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    pv.report_activity(&cred()).await.unwrap();
    let s = st.lock().unwrap();
    // zcode-upstream.ts:397：事件集就是这两条，各一条独立 POST
    assert_eq!(
        s.events,
        vec!["app_launch", "app_daily_active"],
        "事件集与顺序"
    );
    assert_eq!(
        s.event_bodies.len(),
        2,
        "每个事件独立一次上报（不是批量 events 数组）"
    );
    let body = &s.event_bodies[0];
    assert_eq!(body["event"], "app_launch", "字段名是 event（单数）");
    assert!(
        body["device_mid"].as_str().is_some_and(|m| !m.is_empty()),
        "带 device_mid"
    );
    assert_eq!(body["platform"], "win32");
    assert!(body["app_version"].as_str().is_some_and(|v| !v.is_empty()));
}

#[tokio::test]
async fn preview_lists_plans_sorted_with_grant_units() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let plans = pv.claim_preview(&cred()).await.unwrap();
    assert_eq!(plans.len(), 2);
    // priority 降序（先领高优先级）
    assert_eq!(plans[0].plan_id, "plan-glm-pro-14d", "高优先级在前");
    assert_eq!(plans[0].name, "GLM Coding Pro · 14 天");
    assert_eq!(
        plans[0].tokens, 100_000_000,
        "量取自 entitlements[].grant_units"
    );
    assert_eq!(plans[1].plan_id, "plan-low");
    let q = st.lock().unwrap().preview_query.clone().unwrap();
    assert!(
        q.contains("app_version") && q.contains("platform"),
        "preview 需带 query：{q}"
    );
}

#[tokio::test]
async fn claim_without_captcha_returns_need_captcha() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let out = pv
        .submit_claim(&cred(), "plan-glm-pro-14d", None)
        .await
        .unwrap();
    assert!(
        matches!(out, ClaimOutcome::NeedCaptcha),
        "先探后取：{out:?}"
    );
    // 第二次带头 → 成功
    let cap = valid_captcha();
    let out2 = pv
        .submit_claim(&cred(), "plan-glm-pro-14d", Some(&cap))
        .await
        .unwrap();
    assert!(matches!(out2, ClaimOutcome::Claimed { .. }), "{out2:?}");
    let headers = st.lock().unwrap().claim_headers[1].clone();
    assert!(headers.contains_key("x-aliyun-captcha-verify-param"));
    assert!(headers.contains_key("x-aliyun-captcha-verify-region"));
}

#[tokio::test]
async fn degraded_captcha_param_is_not_sent() {
    // SDK 降级产物（长度够但非 base64-JSON）：发了必 3007，本地直接拒发
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let before = st.lock().unwrap().claim_bodies.len();
    let cap = CaptchaParam {
        param: "x".repeat(220),
        region: "cn-hangzhou".into(),
    };
    let out = pv
        .submit_claim(&cred(), "plan-glm-pro-14d", Some(&cap))
        .await
        .unwrap();
    assert!(
        matches!(out, ClaimOutcome::NeedCaptcha),
        "降级 param → NeedCaptcha：{out:?}"
    );
    assert_eq!(st.lock().unwrap().claim_bodies.len(), before, "不发请求");
}

#[tokio::test]
async fn claim_1003_is_idempotent_success_and_1005_is_cooldown() {
    let (base, _) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let cap = valid_captcha();
    let out = pv.submit_claim(&cred(), "p1003", Some(&cap)).await.unwrap();
    assert!(matches!(out, ClaimOutcome::AlreadyClaimed));
    let out2 = pv.submit_claim(&cred(), "p1005", Some(&cap)).await.unwrap();
    match out2 {
        ClaimOutcome::Cooldown { next_at } => assert!(next_at.contains("2026-10-08")),
        other => panic!("expect Cooldown, got {other:?}"),
    }
}

#[tokio::test]
async fn claim_3012_maps_to_cooldown_error() {
    // 风控有账号冷却惩罚（30min→24h→停用），绝不连续重试
    let (base, _) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let cap = valid_captcha();
    let err = pv
        .submit_claim(&cred(), "p3012", Some(&cap))
        .await
        .unwrap_err();
    match &err {
        gateway_core::provider::ProviderError::RateLimited {
            retry_after_secs, ..
        } => {
            assert_eq!(*retry_after_secs, Some(1800), "3012 账号冷却 30 分钟");
        }
        other => panic!("expect RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn claim_daily_orchestrates_report_preview_claim() {
    let (base, st) = spawn().await;
    let pv = ZcodeProvider::new(base);
    let out = pv.claim_daily(&cred()).await.unwrap();
    assert!(
        matches!(out, ClaimOutcome::NeedCaptcha),
        "首轮无验证码应走到先探：{out:?}"
    );
    assert!(
        !st.lock().unwrap().events.is_empty(),
        "claim_daily 必须先补激活事件"
    );
    assert_eq!(st.lock().unwrap().claim_bodies.len(), 1);
    // 先探的是 priority 最高的那一档
    let pid = st.lock().unwrap().claim_bodies[0]["plan_id"].clone();
    assert_eq!(pid, json!("plan-glm-pro-14d"), "先领高优先级档");
}
