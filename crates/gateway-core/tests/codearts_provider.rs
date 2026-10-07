//! T4.12 codearts provider（缝 2：stub 上游）。
//! 行为来源：reference §4.1——
//! - SDK-HMAC-SHA256 签名：七段式 canonical request
//! - `Agent-Type: PromptCenter`/`X-Language: zh-cn` **签名后追加**
//!   （+ `Chat-Id`/`Session-Id`/`lang:en` 同样签名后追加）
//! - x-security-token 是**固定签名头**（在场必须进签名）
//! - benefit 模型 `maas_type: benefit` 头参与签名
//! - 429 判据锚定独立数字（裸子串会命中 4291——额度码）
//! - 排队（TM.00001041）10s 重试；额度（InferHub.4291.200）立即失败

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use gateway_core::openai::ChatRequest;
use gateway_core::provider::Provider;
use gateway_core::providers::codearts::{
    is_429_standalone, is_benefit_model, is_quota_exhausted, is_queued, sdk_hmac_sha256_sign,
    CodeartsProvider,
};
use gateway_core::Credential;
use serde_json::{json, Value};

#[test]
fn hmac_sign_produces_correct_structure() {
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("host".to_string(), "example.com".to_string()),
        ("x-sdk-content-sha256".to_string(), "abc123".to_string()),
        ("x-sdk-date".to_string(), "20261007T120000Z".to_string()),
    ];
    let sig = sdk_hmac_sha256_sign("POST", "/api/v2/chat/completions", "", &headers, b"{}", "test-secret", "test-ak");
    assert!(sig.starts_with("SDK-HMAC-SHA256 Access=test-ak,"), "{sig}");
    assert!(sig.contains("SignedHeaders=content-type;host;x-sdk-content-sha256;x-sdk-date"), "{sig}");
    assert!(sig.contains("Signature="), "{sig}");
    // 签名是 64 位 hex
    let sig_part = sig.rsplit('=').next().unwrap();
    assert_eq!(sig_part.len(), 64, "SHA-256 hex：{sig_part}");
}

#[test]
fn hmac_sign_deterministic() {
    let headers = vec![
        ("host".to_string(), "h".to_string()),
        ("x-sdk-date".to_string(), "d".to_string()),
    ];
    let s1 = sdk_hmac_sha256_sign("GET", "/", "", &headers, b"", "key", "ak");
    let s2 = sdk_hmac_sha256_sign("GET", "/", "", &headers, b"", "key", "ak");
    assert_eq!(s1, s2);
    // 不同 key 不同签名
    let s3 = sdk_hmac_sha256_sign("GET", "/", "", &headers, b"", "other", "ak");
    assert_ne!(s1, s3);
}

#[test]
fn security_token_is_a_signed_header_when_present() {
    // §4.1 签名头固定 host/x-sdk-date/x-sdk-content-sha256/x-security-token
    // （+非 GET 的 content-type）——带临时凭据时 token 必须进 SignedHeaders。
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("host".to_string(), "example.com".to_string()),
        ("x-sdk-content-sha256".to_string(), "abc".to_string()),
        ("x-sdk-date".to_string(), "20261007T120000Z".to_string()),
        ("x-security-token".to_string(), "sts-token-1".to_string()),
    ];
    let sig = sdk_hmac_sha256_sign("POST", "/api/v2/chat/completions", "", &headers, b"{}", "s", "ak");
    assert!(
        sig.contains("SignedHeaders=content-type;host;x-sdk-content-sha256;x-sdk-date;x-security-token"),
        "token 进签名（字母序）：{sig}"
    );
    // token 值变化 → 签名变化（参与 canonical request）
    let mut other = headers.clone();
    other[4].1 = "sts-token-2".to_string();
    let sig2 = sdk_hmac_sha256_sign("POST", "/api/v2/chat/completions", "", &other, b"{}", "s", "ak");
    assert_ne!(sig, sig2, "token 值参与签名计算");
}

#[test]
fn four29_anchor_does_not_hit_4291() {
    // "4291" 不应命中 429 判据（是额度码不是限流码）
    assert!(!is_429_standalone("code 4291 insufficient"), "4291 不含独立 429");
    assert!(is_429_standalone("rate limit 429 too many"), "独立 429 命中");
    assert!(is_429_standalone("HTTP 429"), "HTTP 429 命中");
    assert!(!is_429_standalone("no code here"), "无 429 不命中");
    assert!(is_429_standalone("429"), "整串恰为 429 命中");
    assert!(is_429_standalone("x429"), "词首边界命中");
}

#[test]
fn quota_exhausted_vs_queued() {
    assert!(is_quota_exhausted("InferHub.4291.200 insufficient quota"));
    // §4.1：额度码子串 4291 **或** insufficient quota 文案兜底——两者其一即可
    assert!(is_quota_exhausted("code 4291"), "子串 4291 单独成立");
    assert!(is_quota_exhausted("Insufficient Quota for user"), "文案兜底（大小写不敏感）");
    assert!(!is_quota_exhausted("rate limit 429"));
    assert!(is_queued("TM.00001041 queue full"));
    assert!(is_queued("InferHub.ModelArts.81111.429"));
    assert!(!is_queued("normal response"));
}

#[test]
fn benefit_model_static_set() {
    assert!(is_benefit_model("glm-5.3-flash"));
    assert!(is_benefit_model("deepseek-v4.1-flash"));
    assert!(!is_benefit_model("glm-5.2"));
    assert!(!is_benefit_model("deepseek-v4-flash"));
}

// ── 缝 2：stub 上游（签名头族 + SSE/JSON 双形态 complete） ──

#[derive(Default)]
struct Cap {
    chat_headers: Mutex<Option<HeaderMap>>,
    chat_bodies: Mutex<Vec<String>>,
    /// 第 n 次响应（SSE 或 JSON）
    responses: Mutex<Vec<String>>,
}

async fn stub_chat(State(cap): State<Arc<Cap>>, h: HeaderMap, b: axum::extract::Request) -> Response {
    *cap.chat_headers.lock().unwrap() = Some(h.clone());
    let bytes = axum::body::to_bytes(b.into_body(), 16 << 20).await.unwrap();
    cap.chat_bodies.lock().unwrap().push(String::from_utf8_lossy(&bytes).to_string());
    let n = cap.chat_bodies.lock().unwrap().len() - 1;
    let queue = cap.responses.lock().unwrap();
    let body = if n < queue.len() {
        queue[n].clone()
    } else {
        concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"codearts 回复\"}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        )
        .to_string()
    };
    (StatusCode::OK, [("content-type", "text/event-stream")], body).into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/api/v2/chat/completions", post(stub_chat))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn cred() -> Credential {
    Credential {
        account_id: "ca1".into(),
        secret: json!({
            "access_key": "AKTEST",
            "secret_key": "SKTEST",
            "security_token": "sts-token-1"
        })
        .to_string(),
    }
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "codearts/glm-5.2",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

/// 普通模型：security-token 进签名，benefit 头不发；
/// Chat-Id/Session-Id/lang 签名后追加（不进 SignedHeaders）。
#[tokio::test]
async fn signed_header_family_on_wire() {
    let (base, cap) = spawn().await;
    let route = gateway_core::route::Route { provider: "codearts".into(), model: "glm-5.2".into() };
    CodeartsProvider::new(base).complete(&cred(), &route, &req()).await.unwrap();
    let h = cap.chat_headers.lock().unwrap().clone().unwrap();
    let authz = h.get("authorization").unwrap().to_str().unwrap();
    assert!(
        authz.contains("SignedHeaders=content-type;host;x-sdk-content-sha256;x-sdk-date;x-security-token"),
        "token 是固定签名头：{authz}"
    );
    assert!(!authz.contains("maas_type"), "非 benefit 模型不带 maas_type：{authz}");
    assert_eq!(h.get("x-security-token").unwrap(), "sts-token-1");
    // 签名后追加族
    assert!(h.get("chat-id").is_some(), "Chat-Id 签名后追加");
    assert!(h.get("session-id").is_some(), "Session-Id 签名后追加");
    assert_eq!(h.get("lang").unwrap(), "en");
    assert_eq!(h.get("agent-type").unwrap(), "PromptCenter");
    assert_eq!(h.get("x-language").unwrap(), "zh-cn");
}

/// benefit 模型：`maas_type: benefit` 头参与签名（缺失回 404 model is not registered）。
#[tokio::test]
async fn benefit_model_carries_signed_maas_type() {
    let (base, cap) = spawn().await;
    let route = gateway_core::route::Route { provider: "codearts".into(), model: "glm-5.3-flash".into() };
    CodeartsProvider::new(base).complete(&cred(), &route, &req()).await.unwrap();
    let h = cap.chat_headers.lock().unwrap().clone().unwrap();
    let authz = h.get("authorization").unwrap().to_str().unwrap();
    assert!(
        authz.contains("SignedHeaders=content-type;host;maas_type;x-sdk-content-sha256;x-sdk-date;x-security-token"),
        "benefit 头进签名（字母序）：{authz}"
    );
    assert_eq!(h.get("maas_type").unwrap(), "benefit");
}

/// complete() 请求恒 stream:true——上游回 SSE 也必须能解（旧实现只解 JSON，
/// 成功响应反而全部报「非 JSON」）。
#[tokio::test]
async fn complete_parses_sse_response() {
    let (base, _) = spawn().await;
    let route = gateway_core::route::Route { provider: "codearts".into(), model: "glm-5.2".into() };
    let out = CodeartsProvider::new(base).complete(&cred(), &route, &req()).await.unwrap();
    assert!(
        out.choices[0].message.content.contains("codearts 回复"),
        "SSE 形态 complete 可解：{:?}",
        out.choices[0].message.content
    );
    assert_eq!(out.usage.prompt_tokens, 3);
}

/// 网关直答/重定向等纯 JSON 形态（无 data: 帧）也兼容。
#[tokio::test]
async fn complete_parses_plain_json_response() {
    let (base, cap) = spawn().await;
    cap.responses.lock().unwrap().push(
        json!({
            "id": "cmpl-1",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "json 回复"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })
        .to_string(),
    );
    let route = gateway_core::route::Route { provider: "codearts".into(), model: "glm-5.2".into() };
    let out = CodeartsProvider::new(base).complete(&cred(), &route, &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("json 回复"));
}

/// 工具调用历史原样带回（§7.1.2 静默丢弃是最大敌人）。
#[tokio::test]
async fn tool_calls_echoed_in_body() {
    let (base, cap) = spawn().await;
    let req: ChatRequest = serde_json::from_value(json!({
        "model": "codearts/glm-5.2",
        "messages": [
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "call_1", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\":\"a\"}" } }
            ]},
            { "role": "tool", "tool_call_id": "call_1", "content": "file body" }
        ]
    }))
    .unwrap();
    let route = gateway_core::route::Route { provider: "codearts".into(), model: "glm-5.2".into() };
    CodeartsProvider::new(base).complete(&cred(), &route, &req).await.unwrap();
    let body = cap.chat_bodies.lock().unwrap()[0].clone();
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["messages"][1]["tool_calls"][0]["id"], json!("call_1"), "tool_calls 原样带回：{body}");
    assert_eq!(v["messages"][2]["tool_call_id"], json!("call_1"), "tool_call_id 原样带回：{body}");
}
