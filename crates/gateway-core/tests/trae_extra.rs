//! T4.19 trae 补全（缝 2：stub 上游）。
//! 行为来源（参考源码行号）：
//! - ExchangeToken 续期（trae-auth.ts:378-432）：body
//!   {ClientID:"en1oxy7wnw8j9n", RefreshToken, ClientSecret:"-", UserID:""}；
//!   refreshToken 每次轮换旧值即刻失效**必须立即回写**（新 rt 空保留旧值）；
//!   expires_at 归一**毫秒字符串**；machine_id/device_id/uid/nickname 完全不动；
//!   401/403 或 2xx 无 accessToken 为终态。
//! - 空响应（200 零可解析事件，含 metadata）→ 同账号重发**一次**；已收到
//!   任何事件则绝不重放（trae-adapter.ts:1280-1289/1694-1699）。
//! - 模型目录（trae-auth.ts:676-711）：POST batch_get_detail_param，
//!   functions 传全部 22 个；白名单整组过滤 + usage=="chat_completion" +
//!   config_switch + is_invisible_to_user 三重条目过滤；多通道择优
//!   （空档位不覆盖有档位；同有档位取白名单靠前者）。
//! - 签到（trae-credits.ts）：status/claim body {}；设备身份按 uid 确定性
//!   派生（sha256("salt:uid"+counterBE32)，X-Device-Id 15 位数字、
//!   X-Market-User-ID UUIDv4、Vscode-Sessionid 64hex）；claim 响应只有
//!   {"code":0} **无数值**须补查 status；9074（可为字符串）冷却 300s
//!   不换设备。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::trae::{
    derive_checkin_device_id, TraeProvider, DEFAULT_OAUTH_BASE,
};
use gateway_core::Credential;
use serde_json::{json, Value};

#[derive(Default, Clone)]
struct Cap {
    hits: Vec<(&'static str, Option<HeaderMap>, Option<Value>)>,
    /// ExchangeToken 响应（可切换 401）
    exchange_unauthorized: bool,
    /// ExchangeToken 自定义响应体（缺省用固定 Result）
    exchange_body: Option<Value>,
    /// chat 响应队列：None=空 body，Some=事件文本
    chat_bodies: Vec<Option<String>>,
    /// chat 强制 HTTP 状态（业务码先于状态码用例）
    chat_forced_status: Option<u16>,
    /// chat 非 200 时的错误体
    chat_error_body: Option<String>,
    models_body: Value,
    status_body: Value,
    claim_body: Value,
}

async fn recorder(cap: &Arc<Mutex<Cap>>, kind: &'static str, h: &HeaderMap, body: Option<Value>) {
    cap.lock().unwrap().hits.push((kind, Some(h.clone()), body));
}

async fn stub_exchange(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap, req: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
    let b: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    recorder(&cap, "exchange", &h, Some(b)).await;
    let c = cap.lock().unwrap();
    if c.exchange_unauthorized {
        return (StatusCode::UNAUTHORIZED, "<html>login expired</html>").into_response();
    }
    if let Some(body) = c.exchange_body.clone() {
        return Json(body).into_response();
    }
    Json(json!({ "Result": {
        "Token": "jwt-new",
        "RefreshToken": "rt-new",
        "TokenExpireAt": 1893456000i64,
        "TokenExpireDuration": 86400
    }})).into_response()
}

async fn stub_chat(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap) -> Response {
    let nth = cap.lock().unwrap().hits.iter().filter(|(k, _, _)| *k == "chat").count();
    recorder(&cap, "chat", &h, None).await;
    let c = cap.lock().unwrap();
    if let Some(code) = c.chat_forced_status {
        let body = c.chat_error_body.clone().unwrap_or_default();
        return (StatusCode::from_u16(code).unwrap(), body).into_response();
    }
    let body = if nth < c.chat_bodies.len() {
        c.chat_bodies[nth].clone()
    } else {
        Some(concat!(
            "event: output\ndata: {\"response\":\"好\"}\n\n",
            "event: done\ndata: {\"finish_reason\":\"stop\"}\n\n",
        ).to_string())
    };
    match body {
        Some(b) => (StatusCode::OK, [("content-type", "text/event-stream")], b).into_response(),
        None => (StatusCode::OK, [("content-type", "text/event-stream")], "").into_response(),
    }
}

async fn stub_models(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap, req: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
    let b: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    recorder(&cap, "models", &h, Some(b)).await;
    let body = cap.lock().unwrap().models_body.clone();
    (StatusCode::OK, Json(body)).into_response()
}

async fn stub_ug(State(cap): State<Arc<Mutex<Cap>>>, h: HeaderMap, req: axum::extract::Request) -> Response {
    let uri = req.uri().to_string();
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
    let b: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let kind: &'static str = if uri.contains("/claim") { "claim" } else { "status" };
    recorder(&cap, kind, &h, Some(b)).await;
    let c = cap.lock().unwrap();
    if uri.contains("/claim") {
        return Json(c.claim_body.clone()).into_response();
    }
    Json(c.status_body.clone()).into_response()
}

async fn spawn() -> (String, Arc<Mutex<Cap>>) {
    let cap = Arc::new(Mutex::new(Cap::default()));
    let app = Router::new()
        .route("/cloudide/api/v3/trae/oauth/ExchangeToken", post(stub_exchange))
        .route("/api/agent/v3/llm_utils_chat", post(stub_chat))
        .route("/api/ide/v1/batch_get_detail_param", post(stub_models))
        .route("/trae/api/v2/ug/checkin_credits/status", post(stub_ug))
        .route("/trae/api/v2/ug/checkin_credits/claim", post(stub_ug))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn pv(base: String) -> TraeProvider {
    TraeProvider::with_agent_base(base.clone(), base)
}

fn cred() -> Credential {
    Credential {
        account_id: "t1".into(),
        secret: json!({
            "access_token": "jwt-old",
            "refresh_token": "rt-old",
            "uid": "u-9527",
            "machine_id": "0123456789abcdef0123456789abcdef",
            "device_id": "fedcba9876543210fedcba9876543210",
            "nickname": "开发者"
        })
        .to_string(),
    }
}

fn req() -> gateway_core::openai::ChatRequest {
    serde_json::from_value(json!({
        "model": "trae/glm-5.2",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route() -> gateway_core::route::Route {
    gateway_core::route::Route { provider: "trae".into(), model: "glm-5.2".into() }
}

// ───────── ExchangeToken ─────────

#[tokio::test]
async fn exchange_token_rotates_refresh_token_and_keeps_identity() {
    let (base, cap) = spawn().await;
    let out = pv(base).refresh(&cred()).await.unwrap();
    let v: Value = serde_json::from_str(&out.secret).unwrap();
    assert_eq!(v["access_token"], json!("jwt-new"));
    assert_eq!(v["refresh_token"], json!("rt-new"), "rt 轮换立即回写");
    assert_eq!(v["machine_id"], json!("0123456789abcdef0123456789abcdef"), "machine_id 不动");
    assert_eq!(v["uid"], json!("u-9527"));
    assert_eq!(v["nickname"], json!("开发者"));
    let exp = v["expires_at"].as_str().unwrap();
    assert!(exp.parse::<u64>().unwrap() > 1_000_000_000_000, "毫秒字符串：{exp}");
    let (_, _, body) = cap.lock().unwrap().hits.iter().find(|(k, _, _)| *k == "exchange").cloned().unwrap();
    let b = body.unwrap();
    assert_eq!(b["ClientID"], json!("en1oxy7wnw8j9n"));
    assert_eq!(b["RefreshToken"], json!("rt-old"));
    assert_eq!(b["ClientSecret"], json!("-"));
    assert_eq!(b["UserID"], json!(""));
}

#[tokio::test]
async fn exchange_html_401_maps_to_credential_error() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().exchange_unauthorized = true;
    let err = pv(base).refresh(&cred()).await.unwrap_err();
    assert!(matches!(err, ProviderError::Credential(_)), "{err:?}");
}

#[test]
fn production_uses_separate_oauth_host() {
    // ExchangeToken 在 OAuth 域 api.trae.com.cn（trae-product.ts:186），
    // 与 UG 域 api.trae.cn 不同源——混用会打错服务器。
    assert_eq!(DEFAULT_OAUTH_BASE, "https://api.trae.com.cn");
    let p = TraeProvider::production();
    let _ = p; // 构造不炸即三域齐备
}

// ───────── 空响应重试 ─────────

#[tokio::test]
async fn zero_event_response_retried_once() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().chat_bodies = vec![None];
    let out = pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("好"));
    let chat_hits = cap.lock().unwrap().hits.iter().filter(|(k, _, _)| *k == "chat").count();
    assert_eq!(chat_hits, 2, "零事件重发一次");
}

#[tokio::test]
async fn events_without_done_not_replayed() {
    let (base, cap) = spawn().await;
    // 有 metadata 事件但无 done：不重放，如实报截断
    cap.lock().unwrap().chat_bodies = vec![Some("event: metadata\ndata: {\"sid\":\"x\"}\n\n".to_string())];
    let err = pv(base).complete(&cred(), &route(), &req()).await.unwrap_err();
    assert!(matches!(err, ProviderError::Upstream(_)), "{err:?}");
    let chat_hits = cap.lock().unwrap().hits.iter().filter(|(k, _, _)| *k == "chat").count();
    assert_eq!(chat_hits, 1, "已收到事件绝不重放");
}

// ───────── 模型目录 ─────────

#[tokio::test]
async fn models_filtered_and_merged_across_channels() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().models_body = json!({ "function_configs": [
        // 白名单通道 + 合规格目
        { "function": "solo_work_lite", "config_info_list": [
            { "config_name": "glm-5.2", "usage": "chat_completion", "config_switch": true, "is_invisible_to_user": false },
            { "config_name": "bad-invisible", "usage": "chat_completion", "config_switch": true, "is_invisible_to_user": true },
            { "config_name": "bad-usage", "usage": "other", "config_switch": true, "is_invisible_to_user": false }
        ]},
        // 非白名单通道 → 整组丢弃
        { "function": "chat", "config_info_list": [
            { "config_name": "chat-only-model", "usage": "chat_completion", "config_switch": true, "is_invisible_to_user": false }
        ]},
        // 同 config_name 另一白名单通道（靠后）→ 后覆盖
        { "function": "solo_agent", "config_info_list": [
            { "config_name": "glm-5.2", "usage": "chat_completion", "config_switch": true, "is_invisible_to_user": false }
        ]}
    ]});
    let models = pv(base).fetch_models(&cred()).await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"glm-5.2"));
    assert!(!ids.contains(&"bad-invisible") && !ids.contains(&"bad-usage"), "三重条目过滤：{ids:?}");
    assert!(!ids.contains(&"chat-only-model"), "非白名单通道整组丢弃：{ids:?}");
    let (_, _, body) = cap.lock().unwrap().hits.iter().find(|(k, _, _)| *k == "models").cloned().unwrap();
    let funcs = body.unwrap()["functions"].as_array().unwrap().clone();
    assert_eq!(funcs.len(), 22, "functions 必须传全部 22 个");
    // 非流式头
    let (_, h, _) = cap.lock().unwrap().hits.iter().find(|(k, _, _)| *k == "models").cloned().unwrap();
    assert_eq!(h.unwrap().get("accept").unwrap(), "application/json");
}

// ───────── 签到 ─────────

#[tokio::test]
async fn checkin_claim_supplements_status_and_derives_stable_device() {
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        c.status_body = json!({ "checked_in": false, "credits": 42, "streak_days": 3 });
        c.claim_body = json!({ "code": 0, "message": "success" });
    }
    let out = pv(base).checkin_claim(&cred()).await.unwrap();
    assert_eq!(out.credits, Some(42), "claim 响应无数值须补查 status");
    assert_eq!(out.streak_days, Some(3));
    let ug: Vec<_> = cap.lock().unwrap().hits.iter().filter(|(k, _, _)| *k == "claim" || *k == "status").cloned().collect();
    assert_eq!(ug.len(), 2, "claim + 补查 status（claim 响应无数值）");
    let (_, h, body) = ug.iter().find(|(k, _, _)| *k == "claim").cloned().unwrap();
    let body = body.unwrap();
    assert!(body.as_object().unwrap().is_empty(), "claim body 须为空对象: {body}");
    let h = h.unwrap();
    let did = h.get("x-device-id").unwrap().to_str().unwrap().to_string();
    assert_eq!(did.len(), 15, "15 位数字：{did}");
    assert!(did.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(h.get("x-lscbd-aid").unwrap(), "787976");
    assert_eq!(h.get("package-type").unwrap(), "stable_cn");
    assert!(h.get("x-market-user-id").is_some());
    assert!(h.get("vscode-sessionid").is_some());
    // 每请求独立的 trace/request id（trae.ts:289-290）
    let trace = h.get("x-tt-trace-id").unwrap().to_str().unwrap();
    assert!(trace.starts_with("00-") && trace.ends_with("-01") && trace.len() == 22, "trace 形态 00-<16hex>-01：{trace}");
    assert!(h.get("x-request-id").is_some(), "X-Request-Id 逐字对齐参考");
    assert_eq!(h.get("sec-fetch-dest").unwrap(), "empty");
    assert_eq!(h.get("sec-fetch-mode").unwrap(), "no-cors");
    assert_eq!(h.get("sec-fetch-site").unwrap(), "none");
    // uid 派生确定性
    assert_eq!(derive_checkin_device_id("u-9527"), did);
    assert_ne!(derive_checkin_device_id("u-other"), did, "每账号互异");
}

#[tokio::test]
async fn checkin_9074_cooldown_without_device_rotation() {
    let (base, cap) = spawn().await;
    cap.lock().unwrap().claim_body = json!({ "code": "9074", "message": "too many" });
    let err = pv(base).checkin_claim(&cred()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_secs, .. } => assert_eq!(retry_after_secs, Some(300)),
        other => panic!("9074 → 300s 冷却：{other:?}"),
    }
    let claims: Vec<_> = cap.lock().unwrap().hits.iter().filter(|(k, _, _)| *k == "claim").cloned().collect();
    assert_eq!(claims.len(), 1, "不换设备重试");
    let d1 = claims[0].1.as_ref().unwrap().get("x-device-id").unwrap().to_str().unwrap();
    assert_eq!(d1, derive_checkin_device_id("u-9527"));
}

#[tokio::test]
async fn checkin_1005_maps_to_plan_limit_cooldown() {
    // 200 + code=1005 → PlanLimit 43200s（classifyTraeCheckinError，
    // trae-credits.ts:78-79）
    let (base, cap) = spawn().await;
    cap.lock().unwrap().claim_body = json!({ "code": 1005, "message": "plan" });
    match pv(base).checkin_claim(&cred()).await.unwrap_err() {
        ProviderError::RateLimited { retry_after_secs, .. } => assert_eq!(retry_after_secs, Some(43_200)),
        other => panic!("1005 → 12h 冷却：{other:?}"),
    }
}

#[tokio::test]
async fn catalog_prefers_fetched_models_over_hardcoded_fallback() {
    // fetch_models 成功后 catalog() 返回远端目录；未拉取时回退硬编码单模型
    let (base, cap) = spawn().await;
    let provider = pv(base);
    let fallback = provider.catalog();
    assert_eq!(fallback.models.len(), 1);
    assert_eq!(fallback.models[0].id, "glm-5.2");
    drop(fallback);

    cap.lock().unwrap().models_body = json!({ "function_configs": [
        { "function": "solo_work_lite", "config_info_list": [
            { "config_name": "glm-5.2", "usage": "chat_completion", "config_switch": true, "is_invisible_to_user": false },
            { "config_name": "kimi-k3", "usage": "chat_completion", "config_switch": true, "is_invisible_to_user": false }
        ]}
    ]});
    provider.fetch_models(&cred()).await.unwrap();
    let catalog = provider.catalog();
    let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"glm-5.2") && ids.contains(&"kimi-k3"), "catalog 用缓存：{ids:?}");
    assert_eq!(catalog.id, "trae");
}

#[tokio::test]
async fn http_error_body_business_code_precedes_status() {
    // 业务码先于状态码（trae-errors.ts:96-103 判定顺序）：HTTP 400 但错误体带
    // {"code":4008} → quota-exceeded 冷却 24h，而不是按 4xx 折 BadRequest。
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        c.chat_forced_status = Some(400);
        c.chat_error_body = Some(r#"{"code":4008,"message":"exceeded the quota"}"#.to_string());
    }
    let err = pv(base).complete(&cred(), &route(), &req()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_secs, .. } => {
            assert_eq!(retry_after_secs, Some(86_400), "4008 先于 HTTP 400：{err:?}")
        }
        other => panic!("body 带 4008 须按业务码分类：{other:?}"),
    }
    // 非 200 不走空响应重发
    let chat_hits = cap.lock().unwrap().hits.iter().filter(|(k, _, _)| *k == "chat").count();
    assert_eq!(chat_hits, 1);
}

#[tokio::test]
async fn http_error_body_string_code_also_classified() {
    // code 的字符串数字形态（readClaimCode 同款口径）同样参与业务码判定
    let (base, cap) = spawn().await;
    {
        let mut c = cap.lock().unwrap();
        c.chat_forced_status = Some(429);
        c.chat_error_body = Some(r#"{"code":"4011","message":"frequency limit"}"#.to_string());
    }
    match pv(base).complete(&cred(), &route(), &req()).await.unwrap_err() {
        ProviderError::RateLimited { retry_after_secs, .. } => {
            assert_eq!(retry_after_secs, Some(60), "4011 冷却 60s（字符串 code 同样参与判定）");
        }
        other => panic!("字符串 code 4011 须按业务码分类：{other:?}"),
    }
}

#[tokio::test]
async fn exchange_without_expiry_fields_drops_stale_expires_at() {
    // 三态（TokenExpireAt/Duration）都拿不到 → expires_at 按「未知」删除，
    // 不得沿用已轮换旧 token 的过期时刻（参考写 ''/JWT exp 兜底）
    let (base, cap) = spawn().await;
    cap.lock().unwrap().exchange_body = Some(json!({ "Result": {
        "Token": "jwt-new2",
        "RefreshToken": "rt-new2"
    }}));
    let mut stale = cred();
    stale.secret = json!({
        "access_token": "jwt-old",
        "refresh_token": "rt-old",
        "uid": "u-9527",
        "expires_at": "1000"
    })
    .to_string();
    let out = pv(base).refresh(&stale).await.unwrap();
    let v: Value = serde_json::from_str(&out.secret).unwrap();
    assert_eq!(v["access_token"], json!("jwt-new2"));
    assert!(v.get("expires_at").is_none(), "无过期字段的续期不得保留旧 expires_at：{v}");
}
