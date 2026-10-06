//! T1.8 缝 1：POST /v1/messages（Anthropic 协议面）。
//! 行为来源：spec「路由与协议」——Claude Code 类客户端直连；
//! 非流式返回 message 形状，流式返回 anthropic SSE 事件序列（无 [DONE]）。

use gateway_core::config::{AuthMode, GatewayConfig, };
use gateway_core::mock::MockProvider;
use gateway_core::registry::{ModelInfo, ProviderCatalog, Registry};
use std::sync::Arc;

async fn spawn(auth: AuthMode) -> gateway_core::server::GatewayHandle {
    let c = GatewayConfig {
        auth,
        model_map: vec![],
        registry: Registry::with(ProviderCatalog { id: "mock".into(), models: vec![ModelInfo { id: "mock-alpha".into() }] }),
    };
    gateway_core::server::start_with(c, vec![Arc::new(MockProvider)]).await.unwrap()
}

#[tokio::test]
async fn non_stream_returns_anthropic_message_shape() {
    let h = spawn(AuthMode::Disabled).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/messages", h.addr))
        .json(&serde_json::json!({
            "model": "mock/mock-alpha", "max_tokens": 100,
            "system": "你是个严谨的助手",
            "messages": [{ "role": "user", "content": "你好" }]
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let b: serde_json::Value = resp.json().await.unwrap();
    assert!(b["id"].as_str().unwrap().starts_with("msg_"));
    assert_eq!(b["type"], "message");
    assert_eq!(b["role"], "assistant");
    assert_eq!(b["model"], "mock/mock-alpha");
    assert_eq!(b["content"][0]["type"], "text");
    assert_eq!(b["content"][0]["text"], "[mock:mock:mock-alpha] 你好");
    assert_eq!(b["stop_reason"], "end_turn");
    assert!(b["usage"]["input_tokens"].as_u64().unwrap() >= 1);
    assert!(b["usage"]["output_tokens"].as_u64().unwrap() >= 1);
    h.shutdown().await;
}

#[tokio::test]
async fn stream_returns_anthropic_event_sequence_without_done() {
    let h = spawn(AuthMode::Disabled).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/messages", h.addr))
        .json(&serde_json::json!({
            "model": "mock/mock-alpha", "max_tokens": 100, "stream": true,
            "messages": [{ "role": "user", "content": "流式" }]
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap().contains("text/event-stream"));
    let body = resp.text().await.unwrap();
    h.shutdown().await;

    let mut events: Vec<(String, serde_json::Value)> = Vec::new();
    let mut cur = String::new();
    for line in body.lines() {
        if let Some(e) = line.strip_prefix("event: ") {
            cur = e.to_string();
        } else if let Some(d) = line.strip_prefix("data: ") {
            events.push((cur.clone(), serde_json::from_str(d).expect("event json")));
        }
    }
    let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names.first(), Some(&"message_start"));
    assert_eq!(names.last(), Some(&"message_stop"));
    assert!(names.contains(&"content_block_delta"));
    assert!(!body.contains("[DONE]"), "anthropic 面没有 [DONE]");

    let text: String = events
        .iter()
        .filter(|(n, _)| n == "content_block_delta")
        .filter_map(|(_, v)| v["delta"]["text"].as_str())
        .collect();
    assert_eq!(text, "[mock:mock:mock-alpha] 流式");

    let has_usage = events.iter().any(|(n, v)| n == "message_delta" && v["usage"]["output_tokens"].as_u64().unwrap_or(0) >= 1);
    assert!(has_usage, "message_delta 应携带 usage");
}

#[tokio::test]
async fn x_api_key_auth_applies_to_messages_face() {
    let key = gateway_core::key::generate_gateway_key();
    let h = spawn(AuthMode::Required(key.clone())).await;
    let client = reqwest::Client::new();
    let no_key = client
        .post(format!("http://{}/v1/messages", h.addr))
        .json(&serde_json::json!({ "model": "mock/mock-alpha", "max_tokens": 10, "messages": [{ "role": "user", "content": "x" }] }))
        .send().await.unwrap();
    assert_eq!(no_key.status(), 401);
    let ok = client
        .post(format!("http://{}/v1/messages", h.addr))
        .header("x-api-key", key)
        .json(&serde_json::json!({ "model": "mock/mock-alpha", "max_tokens": 10, "messages": [{ "role": "user", "content": "x" }] }))
        .send().await.unwrap();
    assert_eq!(ok.status(), 200);
    h.shutdown().await;
}
