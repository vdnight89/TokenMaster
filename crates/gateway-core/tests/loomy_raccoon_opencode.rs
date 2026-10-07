//! T4.13-T4.15 loomy + raccoon + opencode 测试。

// ── loomy ──
// 已在 loomy_provider.rs 覆盖（5 测）

// ── raccoon + opencode ──

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::opencode::{
    classify_opencode_error, generate_project_id, generate_session_id, OpencodeProvider,
};
use gateway_core::providers::raccoon::RaccoonProvider;
use gateway_core::Credential;
use serde_json::json;

#[derive(Default)]
struct Cap {
    chat_headers: Mutex<Option<HeaderMap>>,
    grant_headers: Mutex<Option<HeaderMap>>,
    chat_hits: Mutex<usize>,
}

async fn stub_raccoon_chat(State(cap): State<Arc<Cap>>, h: HeaderMap, _b: axum::extract::Request) -> Response {
    *cap.chat_headers.lock().unwrap() = Some(h.clone());
    *cap.chat_hits.lock().unwrap() += 1;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"raccoon 回复\"}}]}\n\ndata: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
    (StatusCode::OK, [("content-type", "text/event-stream")], sse.to_string()).into_response()
}

async fn stub_raccoon_grant(State(cap): State<Arc<Cap>>, h: HeaderMap) -> Response {
    *cap.grant_headers.lock().unwrap() = Some(h.clone());
    Json(json!({ "granted": true })).into_response()
}

async fn stub_opencode_chat(State(cap): State<Arc<Cap>>, h: HeaderMap, _b: axum::extract::Request) -> Response {
    *cap.chat_headers.lock().unwrap() = Some(h.clone());
    *cap.chat_hits.lock().unwrap() += 1;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"opencode 回复\"}}]}\n\ndata: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
    (StatusCode::OK, [("content-type", "text/event-stream")], sse.to_string()).into_response()
}

async fn spawn_raccoon() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/api/web/llm/v2/chat/completions", post(stub_raccoon_chat))
        .route("/api/web/desktop/v1/login/points/grant", post(stub_raccoon_grant))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

async fn spawn_opencode() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/v1/chat/completions", post(stub_opencode_chat))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn raccoon_cred() -> Credential {
    Credential {
        account_id: "r1".into(),
        secret: json!({ "access_token": "rc-at", "refresh_token": "rc-rt" }).to_string(),
    }
}

fn opencode_cred() -> Credential {
    Credential {
        account_id: "oc1".into(),
        secret: "sk-opencode-key".into(),
    }
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "test/model",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

// ── raccoon tests ──

#[tokio::test]
async fn raccoon_grant_needs_client_platform_header() {
    let (base, cap) = spawn_raccoon().await;
    let granted = RaccoonProvider::new(base).grant_login_points(&raccoon_cred()).await.unwrap();
    assert!(granted);
    let h = cap.grant_headers.lock().unwrap().clone().unwrap();
    assert!(
        h.get("x-client-platform").unwrap().to_str().unwrap().starts_with("desktop-"),
        "X-Client-Platform 必须 desktop-* 前缀"
    );
}

#[tokio::test]
async fn raccoon_infer_basic() {
    let (base, _) = spawn_raccoon().await;
    let route = gateway_core::route::Route { provider: "raccoon".into(), model: "sn-sensenova-6-8-flash".into() };
    let out = RaccoonProvider::new(base)
        .complete(&raccoon_cred(), &route, &req())
        .await
        .unwrap();
    assert!(out.choices[0].message.content.contains("raccoon 回复"));
}

// ── opencode tests ──

#[tokio::test]
async fn opencode_five_headers_no_machine_fingerprint() {
    let (base, cap) = spawn_opencode().await;
    let route = gateway_core::route::Route { provider: "opencode".into(), model: "qwen-3.5-coder".into() };
    OpencodeProvider::new(base).complete(&opencode_cred(), &route, &req()).await.unwrap();
    let h = cap.chat_headers.lock().unwrap().clone().unwrap();
    // 五个 opencode 头
    assert!(h.get("x-opencode-project").is_some());
    assert!(h.get("x-opencode-session").is_some());
    assert!(h.get("x-opencode-request").is_some());
    assert!(h.get("x-opencode-client").is_some());
    // **不发**机器指纹头
    assert!(h.get("x-session-affinity").is_none(), "无 x-session-affinity");
    assert!(h.get("x-session-id").is_none(), "无 X-Session-Id");
}

#[tokio::test]
async fn opencode_session_id_shape() {
    let sid = generate_session_id();
    assert!(sid.starts_with("ses_"), "{sid}");
    assert_eq!(sid.len(), 4 + 12 + 14, "ses_ + 12hex + 14base62 = 30：{sid} (len={})", sid.len());
    // 12 位小写 hex（时间戳段）
    let ts_part = &sid[4..16];
    assert!(ts_part.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

#[test]
fn opencode_project_id_sha1_shape() {
    let pid = generate_project_id("https://github.com/example/repo.git");
    assert_eq!(pid.len(), 40, "sha1 = 40 hex：{pid}");
    assert!(pid.chars().all(|c| c.is_ascii_hexdigit()));
    // 确定性
    assert_eq!(pid, generate_project_id("https://github.com/example/repo.git"));
    assert_ne!(pid, generate_project_id("https://github.com/other/repo.git"));
}

#[test]
fn opencode_error_classification_by_type_name_not_status() {
    // FreeUsageLimitError 可带任意状态码——分类按类型名不按状态码
    assert!(matches!(
        classify_opencode_error(r#"{"type":"FreeUsageLimitError","message":"limit"}"#),
        ProviderError::RateLimited { .. }
    ));
    assert!(matches!(
        classify_opencode_error(r#"{"type":"GoUsageLimitError"}"#),
        ProviderError::RateLimited { .. }
    ));
    assert!(matches!(
        classify_opencode_error("some other error"),
        ProviderError::Upstream(_)
    ));
}
