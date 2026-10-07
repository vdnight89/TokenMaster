//! T4.2b gemini 元数据切片（缝 2：stub 上游）。
//! 行为来源：gemini-project.ts / gemini-credits.ts——
//! - project 动态探测：loadCodeAssist 的 `cloudaicompanionProject`（两级取值），
//!   `aicode-consumers` 只是空兜底；**探测失败 ≠ 探测到空，失败时不发推理**。
//! - 配额：`/v1internal:retrieveUserQuotaSummary`，请求体必须带 `project`
//!   （空对象对部分账号 403 SUBSCRIPTION_REQUIRED）。
//! - **LCA 与配额固定走 sandbox 端点**（原版 baseFor 路由，gemini-credits.ts:31-33；
//!   gemini-project.ts:100-106 的探测序也是 sandbox 先、daily 兜底）；
//!   推理端点轮换序才是 daily 先。
//! - 探测结果跨请求缓存（一轮账号周期只探一次）。

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::gemini::GeminiProvider;
use gateway_core::route::Route;
use gateway_core::Credential;
use serde_json::{json, Value};

/// loadCodeAssist 的可配置行为。
#[derive(Clone)]
enum LoadMode {
    /// 200 + cloudaicompanionProject=该值
    Proj(&'static str),
    /// 200 但无该字段（空兜底路径）
    Empty,
    /// 500（探测失败路径）
    Error,
}

struct Meta {
    load: LoadMode,
    /// retrieveUserQuotaSummary 的响应状态码
    quota_status: u16,
}

#[derive(Default)]
struct Seen {
    load_count: usize,
    quota_count: usize,
    gen_count: usize,
    quota_body: Option<String>,
    quota_headers: Option<HeaderMap>,
    gen_body: Option<String>,
}

async fn stub_load(State(cap): State<Arc<(Meta, Mutex<Seen>)>>) -> Response {
    cap.1.lock().unwrap().load_count += 1;
    match cap.0.load {
        LoadMode::Proj(p) => (StatusCode::OK, Json(json!({ "cloudaicompanionProject": p }))).into_response(),
        LoadMode::Empty => (StatusCode::OK, Json(json!({}))).into_response(),
        LoadMode::Error => (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response(),
    }
}

async fn stub_quota(
    State(cap): State<Arc<(Meta, Mutex<Seen>)>>,
    headers: HeaderMap,
    body: axum::extract::Request,
) -> Response {
    {
        let mut s = cap.1.lock().unwrap();
        s.quota_count += 1;
        s.quota_headers = Some(headers.clone());
    }
    let bytes = axum::body::to_bytes(body.into_body(), 1 << 20).await.unwrap();
    cap.1.lock().unwrap().quota_body = Some(String::from_utf8_lossy(&bytes).to_string());
    match cap.0.quota_status {
        200 => (StatusCode::OK, Json(json!({ "windows": [{ "pct": 42 }] }))).into_response(),
        code => (StatusCode::from_u16(code).unwrap(), Json(json!({ "error": { "code": code } }))).into_response(),
    }
}

async fn stub_generate(State(cap): State<Arc<(Meta, Mutex<Seen>)>>, body: axum::extract::Request) -> Response {
    let bytes = axum::body::to_bytes(body.into_body(), 8 << 20).await.unwrap();
    {
        let mut s = cap.1.lock().unwrap();
        s.gen_count += 1;
        s.gen_body = Some(String::from_utf8_lossy(&bytes).to_string());
    }
    let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"好\"}],\"role\":\"model\"}}]}\n\n";
    ([("content-type", "text/event-stream")], sse).into_response()
}

async fn spawn(load: LoadMode, quota_status: u16) -> (String, Arc<(Meta, Mutex<Seen>)>) {
    let cap = Arc::new((
        Meta { load, quota_status },
        Mutex::new(Seen::default()),
    ));
    let app = Router::new()
        .route("/v1internal:loadCodeAssist", post(stub_load))
        .route("/v1internal:retrieveUserQuotaSummary", post(stub_quota))
        .route("/v1internal:streamGenerateContent", post(stub_generate))
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
        "messages": [{ "role": "user", "content": "打招呼" }]
    }))
    .unwrap()
}

fn route() -> Route {
    Route { provider: "gemini".into(), model: "gemini-3.8-flash".into() }
}

fn cred() -> Credential {
    Credential { account_id: "g1".into(), secret: "oauth-token".into() }
}

fn gen_project(cap: &Arc<(Meta, Mutex<Seen>)>) -> Value {
    let s = cap.1.lock().unwrap();
    let raw = s.gen_body.as_deref().expect("推理请求未发出");
    serde_json::from_str(raw).unwrap()
}

#[tokio::test]
async fn detected_project_flows_into_envelope() {
    let (base, cap) = spawn(LoadMode::Proj("proj-777"), 200).await;
    let out = pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("好"));
    let v = gen_project(&cap);
    assert_eq!(v["project"], json!("proj-777"));
    assert_eq!(cap.1.lock().unwrap().load_count, 1, "推理前应恰好探测一次");
}

#[tokio::test]
async fn empty_detection_falls_back_to_aicode_consumers() {
    let (base, cap) = spawn(LoadMode::Empty, 200).await;
    pv(base).complete(&cred(), &route(), &req()).await.unwrap();
    assert_eq!(gen_project(&cap)["project"], json!("aicode-consumers"));
}

#[tokio::test]
async fn probe_failure_blocks_inference() {
    let (base, cap) = spawn(LoadMode::Error, 200).await;
    let err = pv(base).complete(&cred(), &route(), &req()).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::Upstream(_)),
        "探测失败应报上游错误而非兜底：{err:?}"
    );
    assert_eq!(cap.1.lock().unwrap().gen_count, 0, "探测失败不得发推理");
}

#[tokio::test]
async fn detection_cached_across_requests() {
    let (base, cap) = spawn(LoadMode::Proj("proj-7"), 200).await;
    let p = pv(base);
    p.complete(&cred(), &route(), &req()).await.unwrap();
    p.complete(&cred(), &route(), &req()).await.unwrap();
    assert_eq!(cap.1.lock().unwrap().load_count, 1, "探测结果应跨请求缓存");
    let v = gen_project(&cap);
    assert_eq!(v["project"], json!("proj-7"));
}

#[tokio::test]
async fn quota_summary_posts_project_with_identity_headers() {
    let (base, cap) = spawn(LoadMode::Empty, 200).await;
    let v = pv(base).quota_summary(&cred(), "aicode-consumers").await.unwrap();
    assert_eq!(v["windows"][0]["pct"], json!(42));
    let s = cap.1.lock().unwrap();
    assert_eq!(s.quota_count, 1);
    let qb = s.quota_body.as_deref().unwrap();
    assert!(qb.contains("\"project\":\"aicode-consumers\""), "配额请求体必须带 project（空对象部分账号 403）：{qb}");
    let h = s.quota_headers.as_ref().unwrap();
    assert!(h.get("authorization").unwrap().to_str().unwrap().starts_with("Bearer "));
    assert_eq!(h.get("user-agent").unwrap().to_str().unwrap(), "antigravity/4.3.0 (cmdc-pak)");
}

#[tokio::test]
async fn quota_unauthorized_maps_to_credential_error() {
    let (base, _) = spawn(LoadMode::Empty, 401).await;
    let err = pv(base).quota_summary(&cred(), "aicode-consumers").await.unwrap_err();
    assert!(matches!(err, ProviderError::Credential(_)), "{err:?}");
}

/// LCA 与配额固定 sandbox、推理走 daily（baseFor 路由语义，gemini-credits.ts:31-33）。
/// 双桩：daily 只服务推理并计数；sandbox 只服务 LCA/配额并计数。
#[tokio::test]
async fn lca_and_quota_hit_sandbox_while_inference_hits_daily() {
    #[derive(Default)]
    struct Counts {
        daily_gen: usize,
        daily_meta: usize,
        sandbox_load: usize,
        sandbox_quota: usize,
    }
    let counts = Arc::new(Mutex::new(Counts::default()));

    let c2 = counts.clone();
    let daily = axum::Router::new()
        .route(
            "/v1internal:streamGenerateContent",
            post(move |s: axum::extract::State<Arc<Mutex<Counts>>>| async move {
                s.0.lock().unwrap().daily_gen += 1;
                (
                    [("content-type", "text/event-stream")],
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"好\"}],\"role\":\"model\"}}]}\n\n",
                )
            }),
        )
        .route(
            "/v1internal:loadCodeAssist",
            post(move |s: axum::extract::State<Arc<Mutex<Counts>>>| async move {
                s.0.lock().unwrap().daily_meta += 1;
                (StatusCode::INTERNAL_SERVER_ERROR, "must not be called")
            }),
        )
        .route(
            "/v1internal:retrieveUserQuotaSummary",
            post(move |s: axum::extract::State<Arc<Mutex<Counts>>>| async move {
                s.0.lock().unwrap().daily_meta += 1;
                (StatusCode::INTERNAL_SERVER_ERROR, "must not be called")
            }),
        )
        .with_state(c2);
    let l1 = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let daily_addr = l1.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l1, daily).await.expect("daily serve") });

    let c3 = counts.clone();
    let sandbox = axum::Router::new()
        .route(
            "/v1internal:loadCodeAssist",
            post(move |s: axum::extract::State<Arc<Mutex<Counts>>>| async move {
                s.0.lock().unwrap().sandbox_load += 1;
                Json(json!({ "cloudaicompanionProject": "proj-sb" }))
            }),
        )
        .route(
            "/v1internal:retrieveUserQuotaSummary",
            post(move |s: axum::extract::State<Arc<Mutex<Counts>>>| async move {
                s.0.lock().unwrap().sandbox_quota += 1;
                Json(json!({ "windows": [{ "pct": 7 }] }))
            }),
        )
        .with_state(c3);
    let l2 = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let sandbox_addr = l2.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l2, sandbox).await.expect("sandbox serve") });

    let p = GeminiProvider::new(format!("http://{daily_addr}"), "aicode-consumers".into())
        .with_sandbox_base(format!("http://{sandbox_addr}"));
    let out = p.complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("好"));
    let quota = p.quota_summary(&cred(), "proj-sb").await.unwrap();
    assert_eq!(quota["windows"][0]["pct"], json!(7));

    let c = counts.lock().unwrap();
    assert_eq!(c.daily_gen, 1, "推理走 daily（轮换序首个）");
    assert_eq!(c.daily_meta, 0, "LCA/配额不得打 daily");
    assert_eq!(c.sandbox_load, 1, "LCA 固定 sandbox");
    assert_eq!(c.sandbox_quota, 1, "配额固定 sandbox");
}

/// sandbox LCA 失败时兜底探测 daily（gemini-project.ts:142-172 的逐端点循环）。
#[tokio::test]
async fn lca_falls_back_to_daily_when_sandbox_fails() {
    let sb = axum::Router::new().route(
        "/v1internal:loadCodeAssist",
        post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "sandbox down") }),
    );
    let l1 = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let sb_addr = l1.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l1, sb).await.expect("sb serve") });

    let load_hits = Arc::new(Mutex::new(0usize));
    let h2 = load_hits.clone();
    let daily = axum::Router::new()
        .route(
            "/v1internal:loadCodeAssist",
            post(move |s: axum::extract::State<Arc<Mutex<usize>>>| async move {
                *s.0.lock().unwrap() += 1;
                Json(json!({ "cloudaicompanionProject": "proj-daily" }))
            }),
        )
        .route(
            "/v1internal:streamGenerateContent",
            post(|| async {
                (
                    [("content-type", "text/event-stream")],
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"好\"}],\"role\":\"model\"}}]}\n\n",
                )
            }),
        )
        .with_state(h2);
    let l2 = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let daily_addr = l2.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l2, daily).await.expect("daily serve") });

    let p = GeminiProvider::new(format!("http://{daily_addr}"), "aicode-consumers".into())
        .with_sandbox_base(format!("http://{sb_addr}"));
    let out = p.complete(&cred(), &route(), &req()).await.unwrap();
    assert!(out.choices[0].message.content.contains("好"));
    assert_eq!(*load_hits.lock().unwrap(), 1, "sandbox 挂了应兜底探测 daily");
}
