//! 网关 HTTP 服务：axum 路由 + 鉴权中间件 + 优雅停机。
//! 缝 1 的被测边界——测试进程内 `start_with()` 起真实监听。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::config::{AuthMode, GatewayConfig};
use crate::error::ApiError;
use crate::openai::ChatRequest;
use crate::provider::{Provider, ProviderError};
use crate::route::resolve_model;

#[derive(Clone)]
pub struct AppState {
    pub config: GatewayConfig,
    pub providers: HashMap<String, Arc<dyn Provider>>,
}

pub struct GatewayHandle {
    pub addr: SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl GatewayHandle {
    /// 优雅停机：等待进行中的请求收尾。
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// 在回环随机端口启动网关（无 Provider 注册；鉴权/模型目录类测试用）。
pub async fn start(config: GatewayConfig) -> std::io::Result<GatewayHandle> {
    start_with(config, Vec::new()).await
}

/// 在回环随机端口启动网关并注册 Provider，返回句柄（缝 1 测试入口）。
pub async fn start_with(
    config: GatewayConfig,
    providers: Vec<Arc<dyn Provider>>,
) -> std::io::Result<GatewayHandle> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let providers: HashMap<String, Arc<dyn Provider>> =
        providers.into_iter().map(|p| (p.id().to_string(), p)).collect();
    let state = AppState { config, providers };
    let app = router(state);
    tokio::spawn(async move {
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            let mut rx = shutdown_rx;
            while rx.changed().await.is_ok() {
                if *rx.borrow() {
                    break;
                }
            }
        });
        let _ = serve.await; // 日志系统接入后换成 tracing
    });
    Ok(GatewayHandle { addr, shutdown: shutdown_tx })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/health", get(health))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let data: Vec<serde_json::Value> = state
        .config
        .registry
        .all_models()
        .into_iter()
        .map(|(provider, model)| {
            json!({ "id": format!("{provider}/{model}"), "object": "model", "owned_by": provider })
        })
        .collect();
    Json(json!({ "object": "list", "data": data }))
}

async fn chat_completions(
    State(state): State<AppState>,
    payload: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(req) = payload.map_err(|_| ApiError::Message("invalid or missing JSON body".into()))?;
    let route = resolve_model(&req.model, &state.config.model_map)?;
    let provider = state.providers.get(&route.provider).ok_or(ApiError::ModelNotFound)?.clone();
    let completion = provider.complete(&route, &req).await.map_err(provider_error)?;
    Ok(Json(serde_json::to_value(completion).expect("completion serializes")).into_response())
}

/// Provider 故障映射为对外错误。M2 编排层在此之前完成换号重试；
/// 到达这里的都是重试耗尽后的终态。
fn provider_error(e: ProviderError) -> ApiError {
    match e {
        ProviderError::RateLimited { retry_after_secs, msg } => {
            ApiError::RateLimited { retry_after_secs, msg }
        }
        ProviderError::Credential(msg) => ApiError::Upstream { status: 401, code: "provider_credentials_rejected".into(), msg },
        ProviderError::BadRequest(msg) => ApiError::Message(msg),
        ProviderError::Upstream(msg) => ApiError::Upstream { status: 502, code: "upstream_error".into(), msg },
    }
}

/// 鉴权：Required 模式下接受 `Authorization: Bearer <key>` 或 `x-api-key: <key>`。
async fn auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    let required = match &state.config.auth {
        AuthMode::Disabled => return next.run(req).await,
        AuthMode::Required(k) => k.clone(),
    };
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let api_key = headers
        .get(HeaderName::from_static("x-api-key"))
        .and_then(|v| v.to_str().ok());
    let ok = bearer == Some(required.as_str()) || api_key == Some(required.as_str());
    if ok {
        next.run(req).await
    } else {
        ApiError::InvalidApiKey.into_response()
    }
}
