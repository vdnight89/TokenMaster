//! T4.2a gemini 推理切片（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §3.6/§4.14——
//! POST {base}/v1internal:streamGenerateContent?alt=sse；双层信封每层键字母序；
//! 五个恒定身份头（antigravity/4.3.0 系，x-machine-id/x-vscode-sessionid 为写死占位）；
//! 不发 Accept / x-goog-api-key；模型名必须带档位后缀（默认 -medium）；
//! SSE 帧逐 data 收集 → completion/流式 chunk。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError, StreamChunk};
use gateway_core::providers::gemini::GeminiProvider;
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Stub {
    raw_body: Option<String>,
    headers: Option<HeaderMap>,
}

async fn stub_generate(State(cap): State<Arc<Mutex<Stub>>>, headers: HeaderMap, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    *cap.lock().unwrap() = Stub {
        raw_body: Some(String::from_utf8_lossy(&bytes).to_string()),
        headers: Some(headers.clone()),
    };
    if !headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").starts_with("Bearer ") {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": {"code": 401}}))).into_response();
    }
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"你\"}],\"role\":\"model\"}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"好，世界\"}],\"role\":\"model\"}}],\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":7,\"cachedContentTokenCount\":3}}\n\n",
    );
    (
        [("content-type", "text/event-stream")],
        sse,
    )
        .into_response()
}

async fn spawn() -> (String, Arc<Mutex<Stub>>) {
    let cap = Arc::new(Mutex::new(Stub::default()));
    let app = Router::new()
        .route("/v1internal:streamGenerateContent", post(stub_generate))
        // T4.2b 起 send 前必探测 project；空响应 → 兜底 aicode-consumers
        .route("/v1internal:loadCodeAssist", post(|| async { Json(json!({})) }))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn pv(base: String) -> GeminiProvider {
    GeminiProvider::new(base, "aicode-consumers".into())
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "gemini/gemini-3.8-flash",
        "messages": [
            { "role": "system", "content": "你是助手" },
            { "role": "user", "content": "打招呼" }
        ]
    }))
    .unwrap()
}

fn route() -> Route {
    Route { provider: "gemini".into(), model: "gemini-3.8-flash".into() }
}

fn cred() -> Credential {
    Credential { account_id: "g1".into(), secret: "oauth-token".into() }
}

#[tokio::test]
async fn envelope_is_alphabetical_with_suffix_and_system_instruction() {
    let (base, cap) = spawn().await;
    let out = pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("你好"));
    let s = cap.lock().unwrap();
    let raw = s.raw_body.as_deref().unwrap();
    let v: Value = serde_json::from_str(raw).unwrap();

    // 模型名带档位后缀（准入键）
    assert!(v["model"].as_str().unwrap().starts_with("gemini-3.8-flash-"), "model: {}", v["model"]);
    // 双层信封根键字母序：model < project < request < requestId < userAgent
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "根键须字母序：{keys:?}");
    assert_eq!(v["project"], json!("aicode-consumers"));

    // request 子键字母序，system 抽到 systemInstruction
    let rk: Vec<&str> = v["request"].as_object().unwrap().keys().map(String::as_str).collect();
    let mut rs = rk.clone();
    rs.sort();
    assert_eq!(rk, rs, "request 键须字母序：{rk:?}");
    assert!(v["request"]["systemInstruction"].is_object(), "system 必须抽到顶层 systemInstruction");
    assert_eq!(v["request"]["contents"][0]["role"], json!("user"));
    assert_eq!(v["request"]["contents"][0]["parts"][0]["text"], json!("打招呼"));
}

#[tokio::test]
async fn five_fixed_antigravity_identity_headers_and_no_accept() {
    let (base, cap) = spawn().await;
    pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    let h = cap.lock().unwrap().headers.clone().unwrap();
    assert_eq!(h.get("user-agent").unwrap().to_str().unwrap(), "antigravity/4.3.0 (cmdc-pak)");
    assert_eq!(h.get("x-client-name").unwrap().to_str().unwrap(), "antigravity");
    assert_eq!(h.get("x-client-version").unwrap().to_str().unwrap(), "4.3.0");
    assert_eq!(h.get("x-machine-id").unwrap().to_str().unwrap(), "cmdc-pak");
    assert_eq!(h.get("x-vscode-sessionid").unwrap().to_str().unwrap(), "proxy");
    // dsh 刻意不带 Accept；reqwest 客户端层强加 `Accept: */*` 无法剥离。
    // 协议的真实风险是带 `application/json` 类 Accept 让上游误判返回非 SSE，
    // 故断言校准为「绝不能是 JSON 类 Accept」。
    let accept = h.get("accept").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(!accept.contains("json"), "流式请求不得带 JSON Accept：{accept}");
    assert!(h.get("x-goog-api-key").is_none());
}

#[tokio::test]
async fn usage_comes_from_usage_metadata() {
    let (base, _) = spawn().await;
    let out = pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    // 互斥口径：input = promptTokenCount(12) − cachedContentTokenCount(3) = 9
    assert_eq!(out.usage.prompt_tokens, 9, "缓存命中须从 input 扣除（防双计费）");
    assert_eq!(out.usage.completion_tokens, 7);
}

#[tokio::test]
async fn stream_yields_content_chunks_then_finish() {
    let (base, _) = spawn().await;
    let mut s = pv(base).stream(&cred(), &route(), &req()).await.unwrap();
    let mut text = String::new();
    let mut finish = None;
    while let Some(item) = s.next().await {
        match item.expect("chunk") {
            StreamChunk::Content(t) => text.push_str(&t),
            StreamChunk::Finish { reason, usage } => finish = Some((reason, usage)),
            _ => {}
        }
    }
    assert_eq!(text, "你好，世界");
    let (reason, usage) = finish.expect("must finish");
    assert_eq!(reason, "stop");
    assert_eq!(usage.completion_tokens, 7);
}

#[tokio::test]
async fn unauthorized_maps_to_credential_error() {
    let err = gateway_core::providers::gemini::map_status_error(401, "unauthorized".into());
    assert!(matches!(err, ProviderError::Credential(_)), "{err:?}");
    let err429 = gateway_core::providers::gemini::map_status_error(429, "quota".into());
    assert!(matches!(err429, ProviderError::RateLimited { .. }));
}

#[tokio::test]
async fn envelope_carries_generation_config_and_short_user_agent() {
    let (base, cap) = spawn().await;
    pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    let s = cap.lock().unwrap();
    let raw = s.raw_body.as_deref().unwrap();
    let v: Value = serde_json::from_str(raw).unwrap();
    let gen = &v["request"]["generationConfig"];
    assert_eq!(gen["maxOutputTokens"], json!(64_000), "必发，缺省 64000");
    assert_eq!(gen["thinkingConfig"]["includeThoughts"], json!(true), "恒 true（假关）");
    assert_eq!(gen["thinkingConfig"]["thinkingBudget"], json!(4000), "默认 medium 档 4000");
    assert_eq!(v["userAgent"], json!("antigravity"), "信封 userAgent 是短串");
    let rid = v["requestId"].as_str().unwrap();
    assert!(rid.starts_with("agent/") && rid.split('/').count() == 3, "agent/{{ms}}/{{8hex}}：{rid}");
    assert!(v["request"]["systemInstruction"]["role"] == json!("system"));
}
