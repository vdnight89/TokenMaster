//! T4.11 lobsterai provider（缝 2：stub 上游）。
//! 行为来源：reference §4.4——
//! - `stream` **恒 true**（`stream:false` 回 500）
//! - 请求头**不发腾讯系归属头**（无 X-Domain/X-Product 等）
//! - 余额 `GET /api/user/profile-summary`→`data.totalCreditsRemaining`
//! - 续期请求体含 uuid/first_keyfrom/latest_keyfrom 身份字段（丢失=静默失败）
//! - 终态判定只有 401/403 或业务码 40100/40101

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::lobsterai::LobsteraiProvider;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    chat_headers: Mutex<Option<HeaderMap>>,
    chat_bodies: Mutex<Vec<String>>,
    refresh_body: Mutex<Option<String>>,
}

async fn stub_chat(State(cap): State<Arc<Cap>>, h: HeaderMap, body: axum::extract::Request) -> Response {
    *cap.chat_headers.lock().unwrap() = Some(h.clone());
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    cap.chat_bodies.lock().unwrap().push(String::from_utf8_lossy(&bytes).to_string());
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"lobsterai 回复\"}}]}\n\ndata: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
    (StatusCode::OK, [("content-type", "text/event-stream")], sse.to_string()).into_response()
}

async fn stub_refresh(State(cap): State<Arc<Cap>>, _h: HeaderMap, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    *cap.refresh_body.lock().unwrap() = Some(String::from_utf8_lossy(&bytes).to_string());
    Json(json!({ "access_token": "lb-at2", "refresh_token": "lb-rt2" })).into_response()
}

async fn stub_profile() -> Response {
    Json(json!({
        "data": { "totalCreditsRemaining": 450.5 }
    }))
    .into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/api/proxy/v1/chat/completions", post(stub_chat))
        .route("/api/auth/refresh", post(stub_refresh))
        .route("/api/user/profile-summary", get(stub_profile))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn cred() -> Credential {
    Credential {
        account_id: "l1".into(),
        secret: json!({
            "access_token": "lb-at",
            "refresh_token": "lb-rt",
            "uuid": "user-uuid-123",
            "first_keyfrom": "lobsterai-web",
            "latest_keyfrom": "lobsterai-cli"
        })
        .to_string(),
    }
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "lobsterai/deepseek-v4-flash",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route() -> gateway_core::route::Route {
    gateway_core::route::Route { provider: "lobsterai".into(), model: "deepseek-v4-flash".into() }
}

#[tokio::test]
async fn stream_always_true_in_body() {
    let (base, cap) = spawn().await;
    LobsteraiProvider::new(base).complete(&cred(), &route(), &req()).await.unwrap();
    let body = cap.chat_bodies.lock().unwrap()[0].clone();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["stream"], json!(true), "stream 恒 true（false 回 500）");
}

#[tokio::test]
async fn no_tencent_attribution_headers() {
    let (base, cap) = spawn().await;
    LobsteraiProvider::new(base).complete(&cred(), &route(), &req()).await.unwrap();
    let h = cap.chat_headers.lock().unwrap().clone().unwrap();
    // 不发腾讯系归属头
    assert!(h.get("x-domain").is_none(), "无 X-Domain");
    assert!(h.get("x-product").is_none(), "无 X-Product");
    assert!(h.get("x-product-code").is_none(), "无 X-Product-Code");
    // 自己的头
    assert!(h.get("x-lobsterai-client-version").is_some());
    assert!(h.get("x-lobsterai-client-capabilities").is_some());
}

#[tokio::test]
async fn balance_reads_profile_summary() {
    let (base, _) = spawn().await;
    let bal = LobsteraiProvider::new(base).balance(&cred()).await.unwrap();
    assert!((bal.total_credits_remaining - 450.5).abs() < 0.01);
}

#[tokio::test]
async fn refresh_carries_identity_fields() {
    let (base, cap) = spawn().await;
    let out = LobsteraiProvider::new(base).refresh(&cred()).await.unwrap();
    let v: Value = serde_json::from_str(&out.secret).unwrap();
    assert_eq!(v["access_token"], json!("lb-at2"));
    let rb = cap.refresh_body.lock().unwrap().clone().unwrap();
    let rv: Value = serde_json::from_str(&rb).unwrap();
    assert_eq!(rv["uuid"], json!("user-uuid-123"), "身份字段必须进续期体");
    assert_eq!(rv["first_keyfrom"], json!("lobsterai-web"));
    assert_eq!(rv["latest_keyfrom"], json!("lobsterai-cli"));
}

#[test]
fn terminal_codes() {
    // 40100/40101 也是终态
    assert!(matches!(
        LobsteraiProvider::classify_lobsterai_error(200, r#"{"code":40100}"#),
        ProviderError::Credential(_)
    ));
    assert!(matches!(
        LobsteraiProvider::classify_lobsterai_error(200, r#"{"code":40101}"#),
        ProviderError::Credential(_)
    ));
    // 网络抖动（非 401/403）不是终态
    assert!(matches!(
        LobsteraiProvider::classify_lobsterai_error(500, "server error"),
        ProviderError::Upstream(_)
    ));
}
