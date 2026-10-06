//! T4.1b zcode 流式（缝 2：stub 上游回放 Anthropic SSE 事件流）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md zcode 节——
//! stream=true 时上游以 `event:`/`data:` 行返回 message_start →
//! content_block_delta(text_delta/thinking_delta) → message_delta(usage) →
//! message_stop；Provider 需翻译为 StreamChunk 序列。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError, StreamChunk};
use gateway_core::providers::zcode::ZcodeProvider;
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

/// stub 收到的请求体（校验 stream 标志传给上游）。
#[derive(Default)]
struct Captured {
    body: Option<Value>,
}

async fn stub_stream(State(cap): State<Arc<Mutex<Captured>>>, headers: HeaderMap, body: axum::extract::Request) -> Response {
    use axum::body::Body;
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let auth = headers.get("authorization").and_then(|x| x.to_str().ok()).unwrap_or_default().to_string();
    if auth.ends_with("bad") {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": {"code": 1001}}))).into_response();
    }
    if auth.ends_with("limited") {
        let mut h = HeaderMap::new();
        h.insert("retry-after", "90".parse().unwrap());
        return (StatusCode::TOO_MANY_REQUESTS, h, Json(json!({"error": {"code": 429}}))).into_response();
    }
    *cap.lock().unwrap() = Captured { body: Some(v) };

    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_s1\",\"usage\":{\"input_tokens\":12,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"好\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":34}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(sse))
        .unwrap()
}

/// 带 thinking_delta 的变体（reasoning 增量透传）。
async fn spawn_stub() -> (String, Arc<Mutex<Captured>>) {
    let cap = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route("/api/v1/zcode-plan/anthropic/v1/messages", post(stub_stream))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "zcode/glm-4.7",
        "messages": [{ "role": "user", "content": "写个函数" }]
    }))
    .unwrap()
}

fn route() -> Route {
    Route { provider: "zcode".into(), model: "glm-4.7".into() }
}

async fn collect(pv: &ZcodeProvider, cred: &Credential) -> Vec<StreamChunk> {
    let mut out = Vec::new();
    let mut s = pv.stream(cred, &route(), &req()).await.unwrap();
    while let Some(item) = s.next().await {
        out.push(item.expect("chunk ok"));
    }
    out
}

#[tokio::test]
async fn upstream_receives_stream_flag() {
    let (base, cap) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential { account_id: "a".into(), secret: "jwt".into() };
    let _ = collect(&pv, &cred).await;
    let c = cap.lock().unwrap();
    assert_eq!(c.body.as_ref().unwrap()["stream"], json!(true), "stream 标志必须传给上游");
}

#[tokio::test]
async fn anthropic_sse_translates_to_stream_chunks() {
    let (base, _) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential { account_id: "a".into(), secret: "jwt".into() };
    let chunks = collect(&pv, &cred).await;

    assert!(matches!(chunks.first(), Some(StreamChunk::Role)), "首块 Role：{:?}", chunks.first());
    let content: String = chunks
        .iter()
        .filter_map(|c| match c {
            StreamChunk::Content(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(content, "你好");
    let finish = chunks
        .iter()
        .rev()
        .find_map(|c| match c {
            StreamChunk::Finish { reason, usage } => Some((reason.clone(), *usage)),
            _ => None,
        })
        .expect("must end with Finish");
    assert_eq!(finish.0, "stop", "end_turn 归一为 stop");
    assert_eq!(finish.1.prompt_tokens, 12, "input_tokens 来自 message_start");
    assert_eq!(finish.1.completion_tokens, 34, "output_tokens 来自 message_delta");
    assert_eq!(finish.1.total_tokens, 46);
}

#[tokio::test]
async fn stream_error_mapping_matches_non_stream() {
    let (base, _) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential { account_id: "a".into(), secret: "limited".into() };
    match pv.stream(&cred, &route(), &req()).await {
        Err(ProviderError::RateLimited { retry_after_secs, .. }) => assert_eq!(retry_after_secs, Some(90)),
        Err(other) => panic!("expect RateLimited, got: {other:?}"),
        Ok(_) => panic!("expect RateLimited, got Ok(stream)"),
    }
    let cred401 = Credential { account_id: "a".into(), secret: "bad".into() };
    assert!(matches!(
        pv.stream(&cred401, &route(), &req()).await,
        Err(ProviderError::Credential(_))
    ));
}
