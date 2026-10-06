//! T4.2c thoughtSignature 跨轮回填（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §4.14 要点② / §3.6——
//! - 响应中 thinking part 的 `thoughtSignature` 与同消息后续 `functionCall` 配对，
//!   按「工具名 + 按键名升序的紧凑 JSON」为键存入独立 sigstore；
//! - 请求侧构造 functionCall part 时查表回填；精确 miss 时按工具名最近一次兜底；
//! - 从未见过 → 不带签名；带签请求被 400 拒 → 去签重试一次。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, StreamChunk};
use gateway_core::providers::gemini::GeminiProvider;
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    gen_count: usize,
    last_body: String,
    /// 每次推理请求是否带 thoughtSignature（按时间序）
    had_sigs: Vec<bool>,
}

async fn stub_generate(State(cap): State<Arc<Mutex<Cap>>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let had_sig = raw.contains("thoughtSignature");
    {
        let mut c = cap.lock().unwrap();
        c.gen_count += 1;
        c.last_body = raw.clone();
        c.had_sigs.push(had_sig);
    }
    // 带签一律 400（模拟上游拒签）；无签正常回 functionCall 帧
    if had_sig {
        return (axum::http::StatusCode::BAD_REQUEST, "signature rejected").into_response();
    }
    let frame = json!({
        "candidates": [{"content": {"parts": [
            { "thoughtSignature": "sig-abc" },
            { "functionCall": { "name": "get_weather", "args": { "city": "北京" } } }
        ], "role": "model"}}]
    });
    let sse = format!("data: {frame}\n\n");
    ([("content-type", "text/event-stream")], sse).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/v1internal:streamGenerateContent", post(stub_generate))
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

fn cred() -> Credential {
    Credential { account_id: "g1".into(), secret: "oauth-token".into() }
}

fn route() -> Route {
    Route { provider: "gemini".into(), model: "gemini-3-pro".into() }
}

/// 历史里带 get_weather functionCall 的请求；args_city 控制规范化参数。
fn req_with_args(city: &str) -> ChatRequest {
    serde_json::from_value(json!({
        "model": "gemini/gemini-3-pro",
        "messages": [
            { "role": "user", "content": "天气怎么样" },
            { "role": "assistant", "content": "", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": { "name": "get_weather", "arguments": format!("{{\"city\":\"{city}\"}}") }
            }]},
            { "role": "tool", "tool_call_id": "call_1", "content": "晴" }
        ]
    }))
    .unwrap()
}

#[tokio::test]
async fn signature_recorded_from_response_and_replayed_on_next_request() {
    let (base, cap) = spawn().await;
    let p = pv(base);
    // 第一轮：响应带签名+functionCall → 记录
    let mut s = p.stream(&cred(), &route(), &req_with_args("北京")).await.unwrap();
    while let Some(item) = s.next().await {
        if let StreamChunk::Finish { .. } = item.expect("chunk") {
            break;
        }
    }
    // 第二轮：请求侧 functionCall 应带 thoughtSignature → 400 → 去签重试一次成功
    let out = p.complete(&cred(), &route(), &req_with_args("北京")).await.unwrap();
    assert!(out.choices[0].message.tool_calls.is_some());
    let c = cap.lock().unwrap();
    assert_eq!(
        c.had_sigs,
        vec![false, true, false],
        "轮1无签记录；轮2先带签被拒，再去签重试"
    );
}

#[tokio::test]
async fn unknown_args_falls_back_to_latest_signature_of_same_tool() {
    let (base, cap) = spawn().await;
    let p = pv(base);
    let mut s = p.stream(&cred(), &route(), &req_with_args("北京")).await.unwrap();
    while let Some(item) = s.next().await {
        if let StreamChunk::Finish { .. } = item.expect("chunk") {
            break;
        }
    }
    // 精确键 (get_weather,{city:北京}) miss（args 换上海）→ 按工具名兜底回填最近签名
    let _ = p.complete(&cred(), &route(), &req_with_args("上海")).await;
    let c = cap.lock().unwrap();
    assert_eq!(
        c.had_sigs,
        vec![false, true, false],
        "args 变更后仍须按工具名兜底带签（再被拒去签重试）"
    );
    // 重试后的最终 body 无签名键
    let v: Value = serde_json::from_str(&c.last_body).unwrap();
    let fc = &v["request"]["contents"][1]["parts"][0]["functionCall"];
    assert!(fc.get("thoughtSignature").is_none());
}

#[tokio::test]
async fn no_signature_key_when_never_seen() {
    let (base, cap) = spawn().await;
    let p = pv(base);
    // 不先发过任何记录请求：直接第二轮
    let _ = p.complete(&cred(), &route(), &req_with_args("北京")).await;
    let c = cap.lock().unwrap();
    let v: Value = serde_json::from_str(&c.last_body).unwrap();
    let fc = &v["request"]["contents"][1]["parts"][0]["functionCall"];
    assert!(fc.get("thoughtSignature").is_none(), "从未见过签名不得臆造");
    assert_eq!(c.gen_count, 1, "无签请求不该被 400 拒，也不该重试");
    assert_eq!(c.had_sigs, vec![false]);
}
