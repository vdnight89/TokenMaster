//! T4.9 buddy provider（缝 2：stub 上游）。
//! 行为来源：reference §4.2——
//! - X-Domain **产品优先 `||`**（空串时 `??` 不生效）
//! - 11140 业务码按账号生效（文案说内容审核但不是 AUTH）——30 分钟冷却
//! - 余额双层嵌套 `data.Response.Data.Accounts[].CapacityRemainPrecise`
//! - 续期带 **`X-Refresh-Token` 头**
//! - 登录轮询 11217 = 未就绪继续

use std::sync::{Arc, Mutex};

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::buddy::{BuddyProvider, BUDDY_PRODUCT_CODE};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    chat_headers: Mutex<Option<HeaderMap>>,
    auth_headers: Mutex<Option<HeaderMap>>,
    token_hits: Mutex<usize>,
    /// 响应队列（第 n 次取第 n 条）
    chat_responses: Mutex<Vec<String>>,
    chat_hits: Mutex<usize>,
}

async fn stub_chat(State(cap): State<Arc<Cap>>, h: HeaderMap, _body: axum::extract::Request) -> Response {
    *cap.chat_headers.lock().unwrap() = Some(h.clone());
    let n = *cap.chat_hits.lock().unwrap();
    *cap.chat_hits.lock().unwrap() += 1;
    let queue = cap.chat_responses.lock().unwrap();
    let body = if n < queue.len() {
        queue[n].clone()
    } else {
        concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"buddy 回复\"}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        ).to_string()
    };
    (StatusCode::OK, [("content-type", "text/event-stream")], body).into_response()
}

async fn stub_auth_state() -> Response {
    Json(json!({
        "state": "state-abc",
        "login_url": "https://www.codebuddy.cn/login?state=state-abc"
    }))
    .into_response()
}

async fn stub_auth_token(State(cap): State<Arc<Cap>>, RawQuery(_q): RawQuery) -> Response {
    let n = *cap.token_hits.lock().unwrap();
    *cap.token_hits.lock().unwrap() += 1;
    if n < 2 {
        return Json(json!({ "code": 11217, "message": "not ready" })).into_response();
    }
    Json(json!({
        "data": { "access_token": "bd-at", "refresh_token": "bd-rt" }
    }))
    .into_response()
}

async fn stub_refresh(State(cap): State<Arc<Cap>>, h: HeaderMap, _body: axum::extract::Request) -> Response {
    *cap.auth_headers.lock().unwrap() = Some(h.clone());
    Json(json!({
        "data": { "access_token": "bd-at2", "refresh_token": "bd-rt2" }
    }))
    .into_response()
}

async fn stub_balance() -> Response {
    Json(json!({
        "data": {
            "Response": {
                "Data": {
                    "Accounts": [
                        { "CapacityRemainPrecise": "100.5" },
                        { "CapacityRemainPrecise": "50" },
                        { "CapacityRemainPrecise": "" }
                    ]
                }
            }
        }
    }))
    .into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/v2/chat/completions", post(stub_chat))
        .route("/v2/plugin/auth/state", post(stub_auth_state))
        .route("/v2/plugin/auth/token", get(stub_auth_token))
        .route("/v2/plugin/auth/token/refresh", post(stub_refresh))
        .route("/v2/billing/meter/get-user-resource", post(stub_balance))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn cred() -> Credential {
    Credential {
        account_id: "b1".into(),
        secret: json!({
            "access_token": "bd-at",
            "refresh_token": "bd-rt",
            "domain": "codebuddy.cn"
        })
        .to_string(),
    }
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "buddy/Deepseek-V4.1-Flash",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route() -> gateway_core::route::Route {
    gateway_core::route::Route { provider: "buddy".into(), model: "Deepseek-V4.1-Flash".into() }
}

#[tokio::test]
async fn headers_carry_domain_product_and_auth() {
    let (base, cap) = spawn().await;
    BuddyProvider::new(base).complete(&cred(), &route(), &req()).await.unwrap();
    let h = cap.chat_headers.lock().unwrap().clone().unwrap();
    assert_eq!(h.get("x-domain").unwrap(), "codebuddy.cn", "X-Domain 取凭据 domain");
    assert_eq!(h.get("x-product-code").unwrap(), BUDDY_PRODUCT_CODE);
    assert_eq!(h.get("x-product").unwrap(), "CodeBuddy");
    assert!(h.get("authorization").unwrap().to_str().unwrap().starts_with("Bearer "));
    assert!(h.get("x-agent-purpose").is_some());
}

#[tokio::test]
async fn domain_product_priority_overrides_credential() {
    let (base, cap) = spawn().await;
    // 产品级 domain 覆盖凭据级（|| 语义：产品非空时优先）
    BuddyProvider::new(base)
        .with_domain("override.example.com".into())
        .complete(&cred(), &route(), &req())
        .await
        .unwrap();
    let h = cap.chat_headers.lock().unwrap().clone().unwrap();
    assert_eq!(h.get("x-domain").unwrap(), "override.example.com", "产品优先 ||");
}

#[tokio::test]
async fn login_poll_11217_continues_then_token() {
    let (base, _) = spawn().await;
    let p = BuddyProvider::new(base);
    let (state, url) = p.login_state().await.unwrap();
    assert_eq!(state, "state-abc");
    assert!(url.contains("codebuddy.cn"));
    assert!(p.login_poll(&state).await.unwrap().is_none(), "11217 继续");
    assert!(p.login_poll(&state).await.unwrap().is_none(), "11217 继续");
    let cred = p.login_poll(&state).await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["access_token"], json!("bd-at"));
}

#[tokio::test]
async fn refresh_carries_x_refresh_token_header() {
    let (base, cap) = spawn().await;
    let out = BuddyProvider::new(base).refresh(&cred()).await.unwrap();
    let v: Value = serde_json::from_str(&out.secret).unwrap();
    assert_eq!(v["access_token"], json!("bd-at2"));
    let h = cap.auth_headers.lock().unwrap().clone().unwrap();
    assert!(h.contains_key("x-refresh-token"), "X-Refresh-Token 头必须存在");
    assert_eq!(h.get("x-refresh-token").unwrap(), "bd-rt");
}

#[tokio::test]
async fn balance_sums_capacity_remain_precise() {
    let (base, _) = spawn().await;
    let bal = BuddyProvider::new(base).balance(&cred()).await.unwrap();
    // "100.5" + "50" + ""（空串跳过）= 150.5
    assert!((bal.total_remaining - 150.5).abs() < 0.01);
}

#[tokio::test]
async fn business_11140_maps_to_30min_cooldown() {
    let err = BuddyProvider::classify_buddy_error(200, r#"{"code":11140,"message":"request illegal"}"#);
    match err {
        ProviderError::RateLimited { retry_after_secs, .. } => {
            assert_eq!(retry_after_secs, Some(1800), "30 分钟按账号冷却");
        }
        other => panic!("11140 → RateLimited 不是 {other:?}"),
    }
}

#[tokio::test]
async fn sse_error_frame_11140_also_caught() {
    let (base, cap) = spawn().await;
    cap.chat_responses.lock().unwrap().push("data: {\"code\":11140,\"message\":\"request illegal in SSE\"}\n\n".to_string());
    let err = BuddyProvider::new(base)
        .complete(&cred(), &route(), &req())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::RateLimited { retry_after_secs: Some(1800), .. }),
        "200+SSE 错误帧 11140 也要捕获：{err:?}"
    );
}

/// 刷新调度器走 `Arc<dyn Provider>` 进 trait refresh——必须委托到固有
/// `BuddyProvider::refresh` 而不是递归回 trait 默认实现（旧实现
/// `Provider::refresh(self, cred)` 会无限递归栈溢出）。timeout 兜底：
/// 若回归成递归，这里以超时失败而不是挂死整个测试进程。
#[tokio::test]
async fn trait_refresh_delegates_to_inherent_impl() {
    let (base, cap) = spawn().await;
    let p: Arc<dyn Provider> = Arc::new(BuddyProvider::new(base));
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        p.refresh(&cred()),
    )
    .await
    .expect("trait refresh 不得递归（10s 超时）")
    .unwrap();
    let v: Value = serde_json::from_str(&out.secret).unwrap();
    assert_eq!(v["access_token"], json!("bd-at2"), "token 已轮换");
    assert!(cap.auth_headers.lock().unwrap().is_some(), "确实打到了 refresh 端点");
}

/// 凭据是合法 JSON 但不是对象（如数组）→ Credential 错误而不是 panic。
/// （当前在 refresh_token 读取处即被挡下；as_object_mut 的防御分支保证
/// 未来取值路径变化时也不会 panic。）
#[tokio::test]
async fn refresh_non_object_credential_is_rejected_not_panicking() {
    let (base, _) = spawn().await;
    let bad = Credential { account_id: "b1".into(), secret: r#"[1,2,3]"#.into() };
    let err = BuddyProvider::new(base).refresh(&bad).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::Credential(_)),
        "非对象凭据 → Credential：{err:?}"
    );
}
