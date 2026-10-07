//! T4.16 zcode 订阅双通道（缝 2：stub 上游）。
//! 行为来源：deepseek-harness-codearts zcode-transport.ts:30-129/257-294 +
//! zcode-auth.ts:592-680 + zcode-pool(Rust) oauth.rs:494-594 双参考互证——
//! - 双通道：start-plan（`zcode.z.ai` + `zcode_jwt`，免费积分，优先）vs
//!   coding-plan（`api.z.ai/api/anthropic` + `coding_plan_key_*`，订阅）；
//!   订阅腿**删 HTTP-Referer** 头。
//! - 订阅 key 三步现换（**只 GET 不建**）：getCustomerInfo 挑默认机构/项目
//!   （projectType=="2" 剔除）→ api_keys 列表找 `name=="zcode-api-key"`
//!   （不是 "zcode"）→ `copy/{apiKey}` 取 secretKey → key = "{apiKey}.{secret}"。
//! - 登录成功后**顺手换 key**：换不到不报错（没订阅是常态），按登录渠道写
//!   coding_plan_key_zai / coding_plan_key_bigmodel。
//! - 换腿：**只认 429 + 1005/1113**（两通道额度独立）；401/1002（同凭据救
//!   不了）、3012（风控重试加重惩罚）、3009（并发走退避）都不换；换腿前
//!   确认另一通道可用。
//! - 模型归属：start-plan {glm-5.3-flash, glm-5.2, glm-5-turbo}；
//!   coding-plan {glm-5.3, glm-5.3-flash}；同属两通道时 start-plan 优先。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::zcode::{parse_zcode_cred, resolve_channel_for, ZcodeChannel, ZcodeProvider};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default)]
struct Cap {
    /// generate 命中记录：(url_host_suffix, has_referer, authz)
    gen_hits: Vec<(String, bool, String)>,
    /// 每次生成响应 (状态码, body code)（按次取用）
    gen_status_queue: Vec<(u16, i64)>,
    customer_hits: usize,
    keys_hits: usize,
    copy_hits: usize,
}

async fn stub_customer(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap) -> Response {
    {
        let mut c = cap.lock().unwrap();
        c.customer_hits += 1;
        let _ = h.clone();
    }
    Json(json!({ "data": { "organizations": [
        { "organizationId": "org-1", "organizationName": "默认机构", "projects": [
            { "projectId": "proj-bad", "projectName": "x", "projectType": "2" },
            { "projectId": "proj-9", "projectName": "默认项目", "projectType": "1" }
        ]},
        { "organizationId": "org-2", "organizationName": "另一家", "projects": [
            { "projectId": "proj-other", "projectName": "y", "projectType": "0" }
        ]}
    ]}})).into_response()
}

async fn stub_keys(State(cap): State<Arc<Mutex<Cap>>>) -> Response {
    cap.lock().unwrap().keys_hits += 1;
    Json(json!([
        { "name": "other-key", "apiKey": "ak-other" },
        { "name": "zcode-api-key", "apiKey": "ak-123" }
    ])).into_response()
}

async fn stub_copy(State(cap): State<Arc<Mutex<Cap>>>) -> Response {
    cap.lock().unwrap().copy_hits += 1;
    Json(json!({ "secretKey": "  sk-456 " })).into_response()
}

async fn stub_generate(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap, req: axum::extract::Request) -> Response {
    let uri = req.uri().to_string();
    let authz = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let has_referer = h.contains_key("http-referer") || h.contains_key("referer");
    let (next_status, body_code) = {
        let mut c = cap.lock().unwrap();
        c.gen_hits.push((uri.clone(), has_referer, authz));
        if c.gen_status_queue.is_empty() { (200, 0) } else { c.gen_status_queue.remove(0) }
    };
    if next_status != 200 {
        return (
            StatusCode::from_u16(next_status).unwrap(),
            Json(json!({ "code": body_code, "message": "err" })),
        ).into_response();
    }
    // Anthropic Messages 非流式形态
    Json(json!({
        "id": "msg_1", "role": "assistant",
        "content": [{ "type": "text", "text": "订阅腿回复" }],
        "usage": { "input_tokens": 3, "output_tokens": 2 }
    })).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/api/biz/customer/getCustomerInfo", get(stub_customer))
        .route("/api/biz/v1/organization/{org}/projects/{proj}/api_keys", get(stub_keys))
        .route("/api/biz/v1/organization/{org}/projects/{proj}/api_keys/copy/{key}", get(stub_copy))
        .route("/api/agent/v3/generate", post(stub_generate))
        .route("/api/v1/zcode-plan/anthropic/v1/messages", post(stub_generate))
        .route("/api/anthropic/v1/messages", post(stub_generate))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub serve") });
    (format!("http://{addr}"), cap)
}

fn cred_json(coding_key: Option<&str>) -> Credential {
    let mut m = serde_json::Map::new();
    m.insert("zcode_jwt".into(), json!("jwt-free"));
    m.insert("zai_access_token".into(), json!("oauth-tok"));
    if let Some(k) = coding_key {
        m.insert("coding_plan_key_zai".into(), json!(k));
    }
    Credential { account_id: "z1".into(), secret: Value::Object(m).to_string() }
}

fn req(model: &str) -> ChatRequest {
    serde_json::from_value(json!({
        "model": format!("zcode/{model}"),
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route(model: &str) -> gateway_core::route::Route {
    gateway_core::route::Route { provider: "zcode".into(), model: model.into() }
}

#[test]
fn channel_resolution_prefers_start_plan_and_honors_models() {
    let both = cred_json(Some("key.sub"));
    // glm-5.3 仅订阅承载
    assert_eq!(resolve_channel_for("glm-5.3", &both), ZcodeChannel::CodingPlan);
    // glm-5.3-flash 双通道都有 → start-plan 优先
    assert_eq!(resolve_channel_for("glm-5.3-flash", &both), ZcodeChannel::StartPlan);
    // glm-5.2 仅免费腿
    assert_eq!(resolve_channel_for("glm-5.2", &both), ZcodeChannel::StartPlan);
    // 无订阅 key：glm-5.3 兜底 start-plan（凭据不具备时回退）
    let free_only = cred_json(None);
    assert_eq!(resolve_channel_for("glm-5.3", &free_only), ZcodeChannel::StartPlan);
    // 裸串凭据 = zcode_jwt（历史兼容）
    let bare = Credential { account_id: "z".into(), secret: "raw-jwt".into() };
    let c = parse_zcode_cred(&bare.secret);
    assert_eq!(c.zcode_jwt, "raw-jwt");
    assert!(c.coding_plan_key_zai.is_none());
}

#[tokio::test]
async fn coding_plan_key_resolved_via_three_steps() {
    let (base, cap) = spawn().await;
    let p = ZcodeProvider::new(base.clone()).with_coding_plan_base(base);
    let key = p.resolve_coding_plan_key(&cred_json(None)).await.unwrap().expect("应换到 key");
    assert_eq!(key, "ak-123.sk-456", "key = apiKey.secret（secret 去空白）");
    let c = cap.lock().unwrap();
    assert_eq!((c.customer_hits, c.keys_hits, c.copy_hits), (1, 1, 1));
}

#[tokio::test]
async fn coding_plan_key_none_without_oauth_token() {
    let (base, _) = spawn().await;
    let bare = Credential { account_id: "z".into(), secret: "raw-jwt".into() };
    let key = ZcodeProvider::new(base).resolve_coding_plan_key(&bare).await.unwrap();
    assert!(key.is_none(), "无 oauth token 短路，不发请求");
}

#[tokio::test]
async fn coding_plan_channel_sends_key_and_drops_referer() {
    let (base, cap) = spawn().await;
    let p = ZcodeProvider::new(base.clone()).with_coding_plan_base(base);
    let out = p.complete(&cred_json(Some("ck.secret")), &route("glm-5.3"), &req("glm-5.3")).await.unwrap();
    assert!(out.choices[0].message.content.contains("订阅腿回复"));
    let c = cap.lock().unwrap();
    let (uri, has_referer, authz) = c.gen_hits[0].clone();
    assert!(uri.contains("/api/anthropic/v1/messages"), "订阅腿端点：{uri}");
    assert!(!uri.contains("zcode-plan"), "不是免费腿端点：{uri}");
    assert!(!has_referer, "订阅腿删 HTTP-Referer");
    assert_eq!(authz, "Bearer ck.secret");
}

#[tokio::test]
async fn quota_exhausted_switches_channel_once() {
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        // 第一发：免费腿 429+1005；第二发：订阅腿 200
        c.gen_status_queue = vec![(429, 1005)];
    }
    let p = ZcodeProvider::new(base.clone()).with_coding_plan_base(base);
    let out = p
        .complete(&cred_json(Some("ck.secret")), &route("glm-5.3-flash"), &req("glm-5.3-flash"))
        .await
        .unwrap();
    assert!(out.choices[0].message.content.contains("订阅腿回复"));
    let c = cap.lock().unwrap();
    assert_eq!(c.gen_hits.len(), 2, "换腿重发一次");
    let (first_uri, _, _) = c.gen_hits[0].clone();
    let (second_uri, _, second_authz) = c.gen_hits[1].clone();
    assert!(first_uri.contains("zcode-plan"), "首发免费腿");
    assert!(second_uri.contains("/api/anthropic/v1/messages"), "换订阅腿");
    assert_eq!(second_authz, "Bearer ck.secret");
}

#[tokio::test]
async fn risk_control_3012_never_switches_channel() {
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        // 429 + 3012（风控）：重试会加重冷却惩罚，绝不换腿
        c.gen_status_queue = vec![(429, 3012), (200, 0)];
    }
    let err = ZcodeProvider::new(base.clone()).with_coding_plan_base(base)
        .complete(&cred_json(Some("ck.secret")), &route("glm-5.3-flash"), &req("glm-5.3-flash"))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited { .. }), "3012 原样上抛：{err:?}");
    let c = cap.lock().unwrap();
    assert_eq!(c.gen_hits.len(), 1, "风控不换腿（重试加重惩罚）");
    let _ = err;
}

#[tokio::test]
async fn no_fallback_when_other_channel_unavailable() {
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        c.gen_status_queue = vec![(429, 1005)];
    }
    // 无订阅 key（另一通道不可用）：429+1005 不换腿，如实报错
    let err = ZcodeProvider::new(base.clone()).with_coding_plan_base(base)
        .complete(&cred_json(None), &route("glm-5.3-flash"), &req("glm-5.3-flash"))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::RateLimited { .. }), "原样上抛限流：{err:?}");
    let c = cap.lock().unwrap();
    assert_eq!(c.gen_hits.len(), 1, "另一通道不可用时不白撞 401");
}

#[tokio::test]
async fn login_attaches_coding_plan_key_silently() {
    use axum::extract::Path as AxPath;
    let cap = Arc::new(Mutex::new(Cap::default()));
    let c2 = cap.clone();
    let app = Router::new()
        .route("/api/v1/oauth/cli/init", post(|| async {
            Json(json!({ "flow_id": "flow-12345678", "url": "https://auth" }))
        }))
        .route(
            "/api/v1/oauth/cli/poll/{flow_id}",
            get(move |AxPath(_): AxPath<String>| {
                let c2 = c2.clone();
                async move {
                    let _ = &c2;
                    Json(json!({
                        "status": "ok",
                        "zcodejwttoken": "jwt-1",
                        "zaiAccessToken": "oauth-tok"
                    }))
                }
            }),
        )
        .route("/api/biz/customer/getCustomerInfo", get(stub_customer))
        .route("/api/biz/v1/organization/{org}/projects/{proj}/api_keys", get(stub_keys))
        .route("/api/biz/v1/organization/{org}/projects/{proj}/api_keys/copy/{key}", get(stub_copy))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });

    let p = ZcodeProvider::new(format!("http://{addr}")).with_coding_plan_base(format!("http://{addr}"));
    let cred = p
        .login_with_interval(std::time::Duration::from_millis(10))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["zcode_jwt"], json!("jwt-1"));
    // zai 渠道登录 → 写 coding_plan_key_zai（顺手换，失败也不影响登录本身）
    assert_eq!(v["coding_plan_key_zai"], json!("ak-123.sk-456"));
    assert!(v.get("coding_plan_key_bigmodel").is_none());
}

#[tokio::test]
async fn login_survives_key_exchange_failure() {
    // key 换不到（无 biz 路由→请求 404）→ 登录照常成功，凭据保持 oauth token
    let app = Router::new()
        .route("/api/v1/oauth/cli/init", post(|| async {
            Json(json!({ "flow_id": "flow-87654321", "url": "https://auth" }))
        }))
        .route("/api/v1/oauth/cli/poll/{flow_id}", get(|| async {
            Json(json!({ "status": "ok", "zcodejwttoken": "jwt-2", "zaiAccessToken": "oauth-x" }))
        }));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    let p = ZcodeProvider::new(format!("http://{addr}")).with_coding_plan_base(format!("http://{addr}"));
    let cred = p
        .login_with_interval(std::time::Duration::from_millis(10))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&cred.secret).unwrap();
    assert_eq!(v["zcode_jwt"], json!("jwt-2"), "换不到 key 不报错，登录照常");
    assert!(v.get("coding_plan_key_zai").is_none());
}
