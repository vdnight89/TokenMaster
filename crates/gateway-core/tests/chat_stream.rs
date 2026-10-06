//! T1.6 缝 1：SSE 流式——chunk 序列、[DONE] 结束、usage 汇总。
//! 行为来源：spec「路由与协议」（OpenAI chat.completion.chunk 形态）。

use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::mock::MockProvider;
use gateway_core::registry::{ModelInfo, ProviderCatalog, Registry};
use std::sync::Arc;

async fn spawn() -> gateway_core::server::GatewayHandle {
    let c = GatewayConfig {
        auth: AuthMode::Disabled,
        model_map: vec![("mock-alpha".to_string(), "mock/mock-alpha".to_string())],
        registry: Registry::with(ProviderCatalog {
            id: "mock".into(),
            models: vec![ModelInfo { id: "mock-alpha".into() }],
        }),
    };
    gateway_core::server::start_with(c, vec![Arc::new(MockProvider)]).await.unwrap()
}

/// 收集 SSE 的全部 data 载荷（跳过注释/keep-alive 行）。
async fn sse_data(base: &str) -> Vec<serde_json::Value> {
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": "mock/mock-alpha",
            "stream": true,
            "messages": [{ "role": "user", "content": "流式你好" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .starts_with("text/event-stream"));
    let body = resp.text().await.unwrap();
    let mut out = Vec::new();
    for line in body.lines() {
        if let Some(payload) = line.strip_prefix("data: ") {
            if payload == "[DONE]" {
                return out;
            }
            out.push(serde_json::from_str(payload).expect("chunk json"));
        }
    }
    panic!("SSE 流没有以 data: [DONE] 结束：{body}");
}

#[tokio::test]
async fn stream_chunks_assemble_into_full_content() {
    let h = spawn().await;
    let chunks = sse_data(&format!("http://{}", h.addr)).await;
    assert!(chunks.len() >= 3, "至少 role + 若干 content + finish");

    let first = chunks[0].clone();
    assert_eq!(first["object"], "chat.completion.chunk");
    assert_eq!(first["choices"][0]["delta"]["role"], "assistant");

    let content: String = chunks
        .iter()
        .filter_map(|c| {
            let arr = c["choices"].as_array()?;
            arr.first()?["delta"]["content"].as_str()
        })
        .collect();
    assert_eq!(content, "[mock:mock:mock-alpha] 流式你好");

    assert!(chunks.iter().any(|c| c["choices"][0]["finish_reason"] == "stop"));
    h.shutdown().await;
}

#[tokio::test]
async fn stream_ends_with_usage_chunk() {
    let h = spawn().await;
    let chunks = sse_data(&format!("http://{}", h.addr)).await;
    let usage_chunk = chunks
        .iter()
        .find(|c| !c["usage"].is_null())
        .expect("应有一个携带 usage 的汇总 chunk");
    assert!(usage_chunk["usage"]["total_tokens"].as_u64().unwrap() >= 1);
    assert!(usage_chunk["choices"].as_array().unwrap().is_empty(), "usage chunk 的 choices 为空");
    h.shutdown().await;
}

#[tokio::test]
async fn non_stream_request_still_returns_json_body() {
    let h = spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/chat/completions", h.addr))
        .json(&serde_json::json!({
            "model": "mock/mock-alpha",
            "messages": [{ "role": "user", "content": "非流式" }]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .contains("application/json"));
    h.shutdown().await;
}
