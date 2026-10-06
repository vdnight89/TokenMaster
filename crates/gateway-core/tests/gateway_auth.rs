//! T1.2/T1.4 缝 1：网关鉴权与 /v1/models 的 HTTP 黑盒行为。
//!
//! 行为来源：docs/spec/v1.md「网关对外契约」——
//! 密钥模式：必须携带 `Authorization: Bearer <key>` 或 `x-api-key: <key>`；
//! 禁用模式：匿名放行；未带/错钥返回 OpenAI 形状的 401。

use gateway_core::config::{AuthMode, GatewayConfig};
use gateway_core::key::generate_gateway_key;
use gateway_core::server::GatewayHandle;

async fn spawn(auth: AuthMode) -> (String, GatewayHandle) {
    let handle = gateway_core::server::start(GatewayConfig { auth, ..Default::default() })
        .await
        .expect("gateway start");
    (format!("http://{}", handle.addr), handle)
}

#[tokio::test]
async fn missing_key_is_rejected_with_openai_error_shape() {
    let (base, h) = spawn(AuthMode::Required(generate_gateway_key())).await;
    let resp = reqwest::get(format!("{base}/v1/models")).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(body["error"]["message"].as_str().unwrap().contains("key"));
    h.shutdown().await;
}

#[tokio::test]
async fn wrong_key_is_rejected() {
    let (base, h) = spawn(AuthMode::Required(generate_gateway_key())).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/v1/models"))
        .bearer_auth("sk-tm-deadbeefdeadbeefdeadbeefdeadbeef")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    h.shutdown().await;
}

#[tokio::test]
async fn valid_bearer_key_is_accepted() {
    let key = generate_gateway_key();
    let (base, h) = spawn(AuthMode::Required(key.clone())).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert!(body["data"].as_array().unwrap().is_empty());
    h.shutdown().await;
}

#[tokio::test]
async fn valid_x_api_key_is_accepted_for_anthropic_clients() {
    let key = generate_gateway_key();
    let (base, h) = spawn(AuthMode::Required(key.clone())).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/v1/models"))
        .header("x-api-key", key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    h.shutdown().await;
}

#[tokio::test]
async fn disabled_mode_allows_anonymous() {
    let (base, h) = spawn(AuthMode::Disabled).await;
    let resp = reqwest::get(format!("{base}/v1/models")).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    h.shutdown().await;
}
