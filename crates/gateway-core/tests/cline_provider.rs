//! T4.7 cline provider（缝 2：stub 上游）。
//! 行为来源：reference §4.8 + cline-product.ts/cline-auth.ts——
//! - `Authorization: Bearer workos:<jwt>` 前缀不可剥 + 客户端归属头
//! - 限流三分类：429 Daily free（人类可读时长）/ 429 其它 / 402 不冷却
//! - 免费模型端点无需认证

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::openai::ChatRequest;
use gateway_core::provider::{Provider, ProviderError};
use gateway_core::providers::cline::{ClineProvider, CLINE_TOKEN_PREFIX};
use gateway_core::Credential;
use serde_json::json;

#[derive(Default)]
struct Cap {
    headers: Mutex<Option<HeaderMap>>,
    /// chat 响应：按次切换（第 n 次取第 n 条）
    responses: Mutex<Vec<(u16, String)>>,
    hits: Mutex<usize>,
}

async fn stub_chat(State(cap): State<Arc<Cap>>, h: HeaderMap, body: axum::extract::Request) -> Response {
    *cap.headers.lock().unwrap() = Some(h.clone());
    let n = *cap.hits.lock().unwrap();
    *cap.hits.lock().unwrap() += 1;
    let _ = axum::body::to_bytes(body.into_body(), 1 << 20).await;
    let queue = cap.responses.lock().unwrap();
    let (status, body) = if n < queue.len() {
        queue[n].clone()
    } else {
        (200, serde_json::json!({
            "id": "cmpl-1", "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "cline 回复"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5}
        }).to_string())
    };
    (StatusCode::from_u16(status).unwrap(), body).into_response()
}

async fn stub_free_models() -> Response {
    Json(json!({ "free": ["cline-free/mimo-v2.6-flash", "cline-free/glm-5.3-flash"] })).into_response()
}

async fn spawn() -> (String, Arc<Cap>) {
    let cap = Arc::new(Cap::default());
    let app = Router::new()
        .route("/api/v1/chat/completions", post(stub_chat))
        .route("/api/v1/ai/cline/recommended-models", get(stub_free_models))
        .with_state(cap.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.expect("stub") });
    (format!("http://{addr}"), cap)
}

fn cred() -> Credential {
    Credential {
        account_id: "c1".into(),
        secret: json!({
            "access_token": format!("{CLINE_TOKEN_PREFIX}eyJhbGciOi.test.jwt"),
            "refresh_token": "rt-1",
            "account_id": "user-123"
        })
        .to_string(),
    }
}

fn req() -> ChatRequest {
    serde_json::from_value(json!({
        "model": "cline/claude-sonnet-4-6",
        "messages": [{ "role": "user", "content": "hi" }]
    }))
    .unwrap()
}

fn route() -> gateway_core::route::Route {
    gateway_core::route::Route { provider: "cline".into(), model: "claude-sonnet-4-6".into() }
}

#[tokio::test]
async fn headers_carry_workos_prefix_and_client_identity() {
    let (base, cap) = spawn().await;
    ClineProvider::new(base).complete(&cred(), &route(), &req()).await.unwrap();
    let h = cap.headers.lock().unwrap().clone().unwrap();
    let authz = h.get("authorization").unwrap().to_str().unwrap();
    assert!(authz.starts_with(&format!("Bearer {CLINE_TOKEN_PREFIX}")), "workos: 前缀不可剥：{authz}");
    assert_eq!(h.get("http-referer").unwrap(), "https://cline.bot");
    assert_eq!(h.get("x-title").unwrap(), "Cline");
    assert_eq!(h.get("x-client-type").unwrap(), "cline-sdk");
}

#[tokio::test]
async fn daily_free_limit_parses_human_readable_duration() {
    let (base, cap) = spawn().await;
    cap.responses.lock().unwrap().push((
        429,
        r#"{"error":{"message":"Daily free limit reached for claude-sonnet-4-6. Try again in 19h 39m"}}"#.to_string(),
    ));
    let err = ClineProvider::new(base).complete(&cred(), &route(), &req()).await.unwrap_err();
    match err {
        ProviderError::RateLimited { retry_after_secs, .. } => {
            assert_eq!(retry_after_secs, Some(19 * 3600 + 39 * 60), "人类可读时长 19h 39m");
        }
        other => panic!("429 Daily free → RateLimited：{other:?}"),
    }
}

#[tokio::test]
async fn payment_required_no_cooldown() {
    let err = ClineProvider::classify_cline_error(402, "Insufficient credits");
    assert!(
        matches!(err, ProviderError::Credential(ref m) if m.contains("充值")),
        "402 → 不冷却换号：{err:?}"
    );
}

#[tokio::test]
async fn region_restriction_403() {
    let err = ClineProvider::classify_cline_error(403, "Access restricted in your region");
    assert!(matches!(err, ProviderError::Credential(_)), "{err:?}");
}

#[tokio::test]
async fn free_models_no_auth_needed() {
    let (base, _) = spawn().await;
    let models = ClineProvider::new(base).free_models().await.unwrap();
    assert_eq!(models.len(), 2);
    assert!(models.iter().any(|m| m.contains("mimo-v2.6-flash")));
}

#[test]
fn human_duration_parser() {
    let p = |s: &str| ClineProvider::parse_human_duration(s);
    assert_eq!(p("Try again in 19h 39m"), Some(19 * 3600 + 39 * 60));
    assert_eq!(p("in 45m"), Some(45 * 60));
    assert_eq!(p("in 2h"), Some(7200));
    assert_eq!(p("no duration"), None);
}

/// 刷新调度器走 `Arc<dyn Provider>` 进 trait refresh——必须委托到固有
/// `ClineProvider::refresh`。stub 不带 /api/v1/auth/refresh 路由（404 →
/// Upstream），若旧实现递归回 trait 默认实现则应得到 BadRequest；
/// timeout 兜底防递归挂死测试进程。
#[tokio::test]
async fn trait_refresh_delegates_to_inherent_impl() {
    let (base, _) = spawn().await;
    let p: Arc<dyn Provider> = Arc::new(ClineProvider::new(base));
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        p.refresh(&cred()),
    )
    .await
    .expect("trait refresh 不得递归（10s 超时）");
    // stub 未实现 refresh 端点（404）：走固有实现 → Upstream“http 404”。
    // 若回归成 trait 默认实现 → BadRequest（可判别）。
    let err = out.unwrap_err();
    assert!(
        matches!(err, ProviderError::Upstream(ref m) if m.contains("404")),
        "trait refresh 应回固有实现的 Upstream(404)，得到 {err:?}"
    );
}
