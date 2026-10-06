//! T4.1a zcode Provider（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md zcode 节——
//! start-plan 通道 POST {base}/api/v1/zcode-plan/anthropic/v1/messages，
//! Bearer JWT + 身份头（anthropic-version/User-Agent ZCode/x-platform 等）；
//! 401→Credential；429→RateLimited(Retry-After)。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::zcode::ZcodeProvider;
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Captured {
    body: Option<Value>,
    auth: Option<String>,
    anthropic_version: Option<String>,
    ua: Option<String>,
}

/// stub 行为按凭据决定：`bad`→401，`limited`→429(Retry-After:120)，其余→200。
async fn stub_messages(
    State(cap): State<Arc<Mutex<Captured>>>,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let auth = headers
        .get("authorization")
        .and_then(|x| x.to_str().ok())
        .unwrap_or_default()
        .to_string();
    {
        let mut c = cap.lock().unwrap();
        c.body = Some(v.clone());
        c.auth = Some(auth.clone());
        c.anthropic_version = headers.get("anthropic-version").and_then(|x| x.to_str().map(str::to_string).ok());
        c.ua = headers.get("user-agent").and_then(|x| x.to_str().map(str::to_string).ok());
    }
    if auth.ends_with("bad") {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error": {"code": 1001}}))).into_response();
    }
    if auth.ends_with("limited") {
        let mut h = HeaderMap::new();
        h.insert("retry-after", "120".parse().unwrap());
        return (axum::http::StatusCode::TOO_MANY_REQUESTS, h, Json(json!({"error": {"code": 429}})))
            .into_response();
    }
    Json(json!({
        "id": "msg_stub_1", "type": "message", "role": "assistant",
        "model": v["model"].clone(),
        "content": [{ "type": "text", "text": "zcode 回答" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 33, "output_tokens": 7 }
    }))
    .into_response()
}

async fn spawn_stub() -> (String, Arc<Mutex<Captured>>) {
    let cap = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route("/api/v1/zcode-plan/anthropic/v1/messages", post(stub_messages))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "zcode/glm-4.7",
        "messages": [
            { "role": "system", "content": "你是编程助手" },
            { "role": "user", "content": "写个函数" }
        ]
    }))
    .unwrap()
}

fn route() -> Route {
    Route { provider: "zcode".into(), model: "glm-4.7".into() }
}

#[tokio::test]
async fn completes_via_anthropic_protocol_with_identity_headers() {
    let (base, cap) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential { account_id: "zc-1".into(), secret: "jwt-token".into() };
    let out = pv.complete(&cred, &route(), &req()).await.unwrap();
    assert_eq!(out.choices[0].message.content, "zcode 回答");
    assert_eq!(out.usage.prompt_tokens, 33);
    assert_eq!(out.usage.completion_tokens, 7);
    assert_eq!(out.model, "zcode/glm-4.7");

    let c = cap.lock().unwrap();
    let body = c.body.as_ref().unwrap();
    assert_eq!(body["model"], "glm-4.7", "上游收到裸模型名");
    assert!(body["system"].as_str().unwrap().contains("你是编程助手"), "system 抽到顶层字段");
    assert_eq!(body["messages"][0]["role"], "user", "消息数组不再含 system 角色");
    assert_eq!(body["messages"][0]["content"], "写个函数");
    assert!(body["max_tokens"].as_u64().unwrap() > 0, "anthropic 协议要求 max_tokens");
    assert_eq!(c.auth.as_deref(), Some("Bearer jwt-token"));
    assert_eq!(c.anthropic_version.as_deref(), Some("2023-06-01"));
    assert!(c.ua.as_deref().unwrap().starts_with("ZCode/"), "User-Agent 需伪装 ZCode 客户端");
}

#[tokio::test]
async fn catalog_lists_zcode_models() {
    let pv = ZcodeProvider::new("http://127.0.0.1:1".into());
    assert_eq!(pv.id(), "zcode");
    let catalog = pv.catalog();
    let ids: Vec<String> = catalog.models.iter().map(|m| m.id.clone()).collect();
    assert!(ids.contains(&"glm-4.7".to_string()));
    assert!(ids.contains(&"glm-4.7-air".to_string()));
}

#[tokio::test]
async fn unauthorized_maps_to_credential_error() {
    let (base, _) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential { account_id: "a".into(), secret: "bad".into() };
    let err = pv.complete(&cred, &route(), &req()).await.unwrap_err();
    assert!(matches!(err, ProviderError::Credential(_)), "got: {err:?}");
}

#[tokio::test]
async fn rate_limit_maps_retry_after() {
    let (base, _) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential { account_id: "a".into(), secret: "limited".into() };
    let err = pv.complete(&cred, &route(), &req()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_secs, .. } => assert_eq!(retry_after_secs, Some(120)),
        other => panic!("expect RateLimited, got {other:?}"),
    }
}
