//! T4.1b zcode 流式（缝 2：stub 上游回放 Anthropic SSE 事件流）。
//! 行为来源：zcode-anthropic.ts consumeAnthropicSse（448-829）——
//! message_start → content_block_delta(text_delta/thinking_delta/
//! input_json_delta) → message_delta(usage 两处收集) → message_stop；
//! error 事件必须报错（不静默）；有工具调用时 finish 恒 tool_calls；
//! 200 零内容 = 空响应错误（额度/权益形态，不是无声空回复）。

use std::sync::{Arc, Mutex};

use axum::body::Body;
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

async fn stub_stream(
    State(cap): State<Arc<Mutex<Captured>>>,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let auth = headers
        .get("authorization")
        .and_then(|x| x.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if auth.ends_with("bad") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"code": 1001}})),
        )
            .into_response();
    }
    if auth.ends_with("limited") {
        let mut h = HeaderMap::new();
        h.insert("retry-after", "90".parse().unwrap());
        return (
            StatusCode::TOO_MANY_REQUESTS,
            h,
            Json(json!({"error": {"code": 429}})),
        )
            .into_response();
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

/// 工具调用变体：text 块 + tool_use 块（input_json_delta 分片）。
async fn stub_tool_stream() -> Response {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"get_weather\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"北京\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":9}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(sse))
        .unwrap()
}

/// error 事件变体：必须报错，绝不静默当「正常结束」。
async fn stub_error_stream() -> Response {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"upstream exploded\"}}\n\n",
    );
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(sse))
        .unwrap()
}

/// 空流变体：200 但零内容事件（额度/权益形态）。
async fn stub_empty_stream() -> Response {
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        )))
        .unwrap()
}

async fn spawn_stub() -> (String, Arc<Mutex<Captured>>) {
    let cap = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route(
            "/api/v1/zcode-plan/anthropic/v1/messages",
            post(stub_stream),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "zcode/glm-5.3-flash",
        "messages": [{ "role": "user", "content": "写个函数" }]
    }))
    .unwrap()
}

fn route() -> Route {
    Route {
        provider: "zcode".into(),
        model: "glm-5.3-flash".into(),
    }
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
    let cred = Credential {
        account_id: "a".into(),
        secret: "jwt".into(),
    };
    let _ = collect(&pv, &cred).await;
    let c = cap.lock().unwrap();
    assert_eq!(
        c.body.as_ref().unwrap()["stream"],
        json!(true),
        "stream 标志必须传给上游"
    );
}

#[tokio::test]
async fn anthropic_sse_translates_to_stream_chunks() {
    let (base, _) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential {
        account_id: "a".into(),
        secret: "jwt".into(),
    };
    let chunks = collect(&pv, &cred).await;

    assert!(
        matches!(chunks.first(), Some(StreamChunk::Role)),
        "首块 Role：{:?}",
        chunks.first()
    );
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
    assert_eq!(
        finish.1.prompt_tokens, 12,
        "input_tokens 来自 message_start"
    );
    assert_eq!(
        finish.1.completion_tokens, 34,
        "output_tokens 来自 message_delta"
    );
    assert_eq!(finish.1.total_tokens, 46);
}

#[tokio::test]
async fn tool_use_stream_emits_tool_call_deltas() {
    let cap = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route(
            "/api/v1/zcode-plan/anthropic/v1/messages",
            post(|State(c): State<Arc<Mutex<Captured>>>| async move {
                let _ = &c;
                stub_tool_stream().await
            }),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });

    let pv = ZcodeProvider::new(format!("http://{addr}"));
    let cred = Credential {
        account_id: "a".into(),
        secret: "jwt".into(),
    };
    let chunks = collect(&pv, &cred).await;
    let tool_chunks: Vec<(u64, Option<String>, Option<String>, String)> = chunks
        .iter()
        .filter_map(|c| match c {
            StreamChunk::ToolCallDelta {
                index,
                id,
                name,
                arguments,
            } => Some((*index, id.clone(), name.clone(), arguments.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(tool_chunks.len(), 2, "两片参数增量：{:?}", tool_chunks);
    assert_eq!(tool_chunks[0].1.as_deref(), Some("toolu_1"), "首片携带 id");
    assert_eq!(
        tool_chunks[0].2.as_deref(),
        Some("get_weather"),
        "首片携带 name"
    );
    assert_eq!(tool_chunks[0].3, "{\"city\":");
    assert_eq!(tool_chunks[1].1, None, "后续片不重复 id/name");
    assert_eq!(tool_chunks[1].3, "\"北京\"}");
    let finish = chunks
        .iter()
        .rev()
        .find_map(|c| match c {
            StreamChunk::Finish { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("Finish");
    assert_eq!(finish, "tool_calls", "有工具调用 ⇒ finish 恒 tool_calls");
}

#[tokio::test]
async fn error_event_surfaces_as_provider_error() {
    let cap = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route(
            "/api/v1/zcode-plan/anthropic/v1/messages",
            post(|State(c): State<Arc<Mutex<Captured>>>| async move {
                let _ = &c;
                stub_error_stream().await
            }),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });

    let pv = ZcodeProvider::new(format!("http://{addr}"));
    let cred = Credential {
        account_id: "a".into(),
        secret: "jwt".into(),
    };
    let mut s = pv.stream(&cred, &route(), &req()).await.unwrap();
    let mut saw_err = false;
    while let Some(item) = s.next().await {
        if let Err(e) = item {
            saw_err = true;
            assert!(
                e.to_string().contains("upstream exploded"),
                "错误消息透传：{e}"
            );
        }
    }
    assert!(saw_err, "error 事件必须报错，不得静默当正常结束");
}

#[tokio::test]
async fn empty_stream_reports_error_not_silent_finish() {
    let cap = Arc::new(Mutex::new(Captured::default()));
    let app = Router::new()
        .route(
            "/api/v1/zcode-plan/anthropic/v1/messages",
            post(|State(c): State<Arc<Mutex<Captured>>>| async move {
                let _ = &c;
                stub_empty_stream().await
            }),
        )
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });

    let pv = ZcodeProvider::new(format!("http://{addr}"));
    let cred = Credential {
        account_id: "a".into(),
        secret: "jwt".into(),
    };
    let mut s = pv.stream(&cred, &route(), &req()).await.unwrap();
    let mut items = Vec::new();
    while let Some(item) = s.next().await {
        items.push(item);
    }
    assert!(
        items.iter().any(|i| i.is_err()),
        "200 但零内容 ⇒ 空响应错误（额度/权益形态），不是无声空回复：{items:?}"
    );
}

#[tokio::test]
async fn stream_error_mapping_matches_non_stream() {
    let (base, _) = spawn_stub().await;
    let pv = ZcodeProvider::new(base);
    let cred = Credential {
        account_id: "a".into(),
        secret: "limited".into(),
    };
    match pv.stream(&cred, &route(), &req()).await {
        Err(ProviderError::RateLimited {
            retry_after_secs, ..
        }) => assert_eq!(retry_after_secs, Some(90)),
        Err(other) => panic!("expect RateLimited, got: {other:?}"),
        Ok(_) => panic!("expect RateLimited, got Ok(stream)"),
    }
    let cred401 = Credential {
        account_id: "a".into(),
        secret: "bad".into(),
    };
    assert!(matches!(
        pv.stream(&cred401, &route(), &req()).await,
        Err(ProviderError::Credential(_))
    ));
}
