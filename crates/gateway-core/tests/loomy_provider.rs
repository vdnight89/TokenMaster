//! T4.13 loomy provider（缝 2：stub 上游）。
//! 五个独特点（reference §4.9）：
//! 1. 短信验证码登录（唯一）
//! 2. 不能续期（唯一）——只探测
//! 3. 两套认证头：chat `Bearer` / 元数据 `token:`
//! 4. 新手任务纯 API 直领
//! 5. 积分两池：永久 `balance` + 每日 `dailyBalance`

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::Provider;
use gateway_core::providers::loomy::LoomyProvider;
use gateway_core::Credential;
use serde_json::json;

#[derive(Default)]
struct Cap {
    chat_headers: Mutex<Option<HeaderMap>>,
    meta_headers: Mutex<Option<HeaderMap>>,
    chat_hits: Mutex<usize>,
}

async fn stub_chat(State(cap): State<Arc<Cap>>, h: HeaderMap, _body: axum::extract::Request) -> Response {
    *cap.chat_headers.lock().unwrap() = Some(h.clone());
    *cap.chat_hits.lock().unwrap() += 1;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"loomy 回复\"}}]}\n\ndata: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
    (StatusCode::OK, [("content-type", "text/event-stream")], sse.to_string()).into_response()
}

async fn stub_models(State(cap): State<Arc<Cap>>, h: HeaderMap) -> Response {
    *cap.meta_headers.lock().unwrap() = Some(h.clone());
    Json(json!({ "data": [{ "id": "spark-x" }] })).into_response()
}

async fn stub_points(State(cap): State<Arc<Cap>>, h: HeaderMap) -> Response {
    *cap.meta_headers.lock().unwrap() = Some(h.clone());
    Json(json!({
        "balance": 15000.0,
        "dailyBalance": 5000.0
    }))
    .into_response()
}

async fn stub_first_login() -> Response {
    Json(json!({ "alreadyProcessed": false })).into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/chat/completions", post(stub_chat))
        .route("/models", get(stub_models))
        .route("/points/records", get(stub_points))
        .route("/points/first-login", post(stub_first_login))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn cred() -> Credential {
    Credential {
        account_id: "ly1".into(),
        secret: json!({ "session": "ly-session-token" }).to_string(),
    }
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "loomy/spark-x",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route() -> gateway_core::route::Route {
    gateway_core::route::Route { provider: "loomy".into(), model: "spark-x".into() }
}

#[tokio::test]
async fn chat_uses_bearer_meta_uses_token_header() {
    let (base, cap) = spawn().await;
    LoomyProvider::new(base.clone()).complete(&cred(), &route(), &req()).await.unwrap();
    LoomyProvider::new(base).balance(&cred()).await.unwrap();

    // chat 只认 Bearer
    let ch = cap.chat_headers.lock().unwrap().clone().unwrap();
    assert!(ch.get("authorization").unwrap().to_str().unwrap().starts_with("Bearer "));
    assert!(ch.get("token").is_none(), "chat 头不带 token:");

    // 元数据只认 token:
    let mh = cap.meta_headers.lock().unwrap().clone().unwrap();
    assert!(mh.get("token").is_some(), "元数据头有 token:");
    assert!(mh.get("authorization").is_none(), "元数据头不带 Bearer");
}

#[tokio::test]
async fn balance_two_pools() {
    let (base, _) = spawn().await;
    let bal = LoomyProvider::new(base).balance(&cred()).await.unwrap();
    assert!((bal.permanent - 15000.0).abs() < 0.01);
    assert!((bal.daily - 5000.0).abs() < 0.01);
}

#[tokio::test]
async fn first_login_idempotent() {
    let (base, _) = spawn().await;
    let already = LoomyProvider::new(base).first_login(&cred()).await.unwrap();
    assert!(!already, "首次 alreadyProcessed=false");
}

#[tokio::test]
async fn probe_session_alive() {
    let (base, _) = spawn().await;
    let alive = LoomyProvider::new(base).probe_session(&cred()).await.unwrap();
    assert!(alive);
}

#[tokio::test]
async fn no_refresh_endpoint() {
    // loomy 是唯一不能续期的 provider——Provider trait 的 refresh 会返回 BadRequest
    let (base, _) = spawn().await;
    let p = LoomyProvider::new(base);
    // LoomyProvider 没实现 Provider::refresh override，所以走默认 BadRequest
    let err = Provider::refresh(&p, &cred()).await.unwrap_err();
    assert!(
        err.to_string().contains("not support"),
        "loomy 不能续期：{err}"
    );
}
