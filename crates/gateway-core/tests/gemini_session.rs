//! T4.2d gemini sessionId 派生升代（缝 2：stub 上游）。
//! 行为来源：docs/reference/deepseek-harness-codearts.md §4.14 要点④⑤——
//! - sessionId 由 `(project, 首条 user 文本, lane)` 确定性派生（跨实例稳定，
//!   非随机）；lane 语义参考实现未载明，按上游端点 host 近似（端口不参与）。
//! - 服务端按 sessionId 累计 token，1M 超限后该 id **永久 400**（句式
//!   "The input token count (N) exceeds…" 无 context 字样）——升代换新 id
//!   重试一次（一次请求最多一代）；再超限归 CONTEXT_WINDOW_EXCEEDED。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::gemini::GeminiProvider;
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    sessions: Vec<String>,
}

enum GenBehavior {
    /// gen0 请求 400 超限句式，gen>0 正常
    ExceededAtGen0,
    /// 恒 400 超限句式
    AlwaysExceeded,
    /// 恒 400 但非超限句式
    Unrelated400,
    /// 恒正常
    Ok,
}

struct Meta {
    behavior: GenBehavior,
}

async fn stub_generate(State(cap): State<Arc<(Meta, Mutex<Cap>)>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let sid = v["request"]["sessionId"].as_str().unwrap_or_default().to_string();
    let gen = sid.rsplit("-g").next().and_then(|g| g.parse::<u32>().ok()).unwrap_or(0);
    cap.1.lock().unwrap().sessions.push(sid);
    let behavior = &cap.0.behavior;
    let use_400 = match behavior {
        GenBehavior::ExceededAtGen0 => gen == 0,
        GenBehavior::AlwaysExceeded => true,
        GenBehavior::Unrelated400 => true,
        GenBehavior::Ok => false,
    };
    if use_400 {
        let body = if matches!(behavior, GenBehavior::Unrelated400) {
            "bad request for other reasons"
        } else {
            "The input token count (1048576) exceeds the maximum allowed number of tokens"
        };
        return (StatusCode::BAD_REQUEST, body).into_response();
    }
    let frame = json!({
        "candidates": [{"content": {"parts": [{ "text": "好" }], "role": "model"}}]
    });
    ([("content-type", "text/event-stream")], format!("data: {frame}\n\n")).into_response()
}

async fn spawn(behavior: GenBehavior) -> (String, Arc<(Meta, Mutex<Cap>)>) {
    let cap = Arc::new((Meta { behavior }, Mutex::new(Cap::default())));
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

fn req(first_user: &str) -> ChatRequest {
    serde_json::from_value(json!({
        "model": "gemini/gemini-3-pro",
        "messages": [{ "role": "user", "content": first_user }]
    }))
    .unwrap()
}

fn cred() -> Credential {
    Credential { account_id: "g1".into(), secret: "oauth-token".into() }
}

fn route() -> Route {
    Route { provider: "gemini".into(), model: "gemini-3-pro".into() }
}

#[tokio::test]
async fn session_derived_deterministically_and_switches_on_first_user_change() {
    let (base, cap) = spawn(GenBehavior::Ok).await;
    let p = pv(base.clone());
    p.complete(&cred(), &route(), &req("你好")).await.unwrap();
    p.complete(&cred(), &route(), &req("你好")).await.unwrap();
    // 跨实例确定性：新 provider 同因子 → 同 sessionId
    pv(base.clone()).complete(&cred(), &route(), &req("你好")).await.unwrap();
    // 换首条 user 文本 → 新会话
    pv(base).complete(&cred(), &route(), &req("换个话题")).await.unwrap();

    let s = cap.1.lock().unwrap().sessions.clone();
    assert_eq!(s[0], s[1], "同因子同实例须复用同一 sessionId");
    assert_eq!(s[0], s[2], "同因子跨实例须确定性派生同一 sessionId");
    assert_ne!(s[0], s[3], "首条 user 文本变化须切换会话");
}

#[tokio::test]
async fn different_project_switches_session() {
    // 两个实例走不同 project（生产=探测结果差异；此处验证派生纯函数因子）
    let a = gateway_core::providers::gemini::GeminiProvider::derive_session_id("proj-a", "你好", "daily-cloudcode-pa.googleapis.com");
    let b = gateway_core::providers::gemini::GeminiProvider::derive_session_id("proj-b", "你好", "daily-cloudcode-pa.googleapis.com");
    assert_ne!(a, b, "project 是派生因子");
    let c = gateway_core::providers::gemini::GeminiProvider::derive_session_id("proj-a", "你好", "daily-cloudcode-pa.sandbox.googleapis.com");
    assert_ne!(a, c, "lane（端点 host）是派生因子");
    assert!(a.starts_with("sess-"));
}

#[tokio::test]
async fn context_exceeded_bumps_generation_once_and_succeeds() {
    let (base, cap) = spawn(GenBehavior::ExceededAtGen0).await;
    let out = pv(base).complete(&cred(), &route(), &req("你好")).await.unwrap();
    assert!(out.choices[0].message.content.contains("好"));
    let s = cap.1.lock().unwrap().sessions.clone();
    assert_eq!(s.len(), 2, "gen0 被拒后应升代重试一次");
    assert_ne!(s[0], s[1], "升代必须换新 sessionId（服务端按 id 累计，削本地历史无用）");
    assert!(s[1].contains("-g"), "升代形态带代数段：{}", s[1]);
}

#[tokio::test]
async fn persistent_exceed_maps_to_context_window_exceeded() {
    let (base, cap) = spawn(GenBehavior::AlwaysExceeded).await;
    let err = pv(base).complete(&cred(), &route(), &req("你好")).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::ContextWindowExceeded(_)),
        "两次超限须归专属错误而非泛化 400：{err:?}"
    );
    let s = cap.1.lock().unwrap().sessions.clone();
    assert_eq!(s.len(), 2, "一次请求最多升一代");
}

#[tokio::test]
async fn unrelated_400_does_not_bump_generation() {
    let (base, cap) = spawn(GenBehavior::Unrelated400).await;
    let err = pv(base).complete(&cred(), &route(), &req("你好")).await.unwrap_err();
    assert!(
        !matches!(err, ProviderError::ContextWindowExceeded(_)),
        "非超限 400 不得误判升代：{err:?}"
    );
    let s = cap.1.lock().unwrap().sessions.clone();
    assert_eq!(s.len(), 1, "非超限 400 不触发升代重试");
}
