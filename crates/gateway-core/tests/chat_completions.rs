//! T1.5 缝 1：POST /v1/chat/completions 非流式——请求经路由解析后
//! 由目标 Provider 产生 OpenAI 形状的 chat.completion（spec：路由与协议）。

use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::mock::MockProvider;
use gateway_core::registry::{ModelInfo, ProviderCatalog};
use std::sync::Arc;

async fn spawn() -> gateway_core::server::GatewayHandle {
    let c = GatewayConfig {
        auth: AuthMode::Disabled,
        model_map: vec![("mock-alpha".to_string(), "mock/mock-alpha".to_string())],
        registry: gateway_core::registry::Registry::with(ProviderCatalog {
            id: "mock".into(),
            models: vec![ModelInfo { id: "mock-alpha".into() }, ModelInfo { id: "mock-beta".into() }],
        }),
    };
    gateway_core::server::start_with(c, vec![Arc::new(MockProvider)]).await.unwrap()
}

async fn post(base: &str, body: serde_json::Value) -> (reqwest::StatusCode, serde_json::Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap())
}

fn user_msg(content: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "mock/mock-alpha",
        "messages": [{ "role": "user", "content": content }]
    })
}

#[tokio::test]
async fn non_stream_completion_returns_openai_shape() {
    let h = spawn().await;
    let base = format!("http://{}", h.addr);
    let (status, body) = post(&base, user_msg("你好，网关")).await;
    assert_eq!(status, 200);
    assert_eq!(body["object"], "chat.completion");
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(!content.is_empty(), "mock provider must produce content");
    assert_eq!(body["choices"][0]["message"]["role"], "assistant");
    assert!(body["usage"]["total_tokens"].as_u64().unwrap() >= 1);
    assert!(body["id"].as_str().unwrap().starts_with("chatcmpl-"));
    h.shutdown().await;
}

#[tokio::test]
async fn bare_model_name_routes_through_map() {
    let h = spawn().await;
    let base = format!("http://{}", h.addr);
    let mut b = user_msg("裸名");
    b["model"] = "mock-alpha".into();
    let (status, body) = post(&base, b).await;
    assert_eq!(status, 200);
    assert_eq!(body["model"], "mock/mock-alpha");
    h.shutdown().await;
}

#[tokio::test]
async fn unknown_model_returns_404_model_not_found() {
    let h = spawn().await;
    let base = format!("http://{}", h.addr);
    let mut b = user_msg("x");
    b["model"] = "nope/ghost".into();
    let (status, body) = post(&base, b).await;
    assert_eq!(status, 404);
    assert_eq!(body["error"]["code"], "model_not_found");
    h.shutdown().await;
}

#[tokio::test]
async fn invalid_body_returns_400_openai_error() {
    let h = spawn().await;
    let base = format!("http://{}", h.addr);
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .json(&serde_json::json!({ "foo": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    h.shutdown().await;
}
