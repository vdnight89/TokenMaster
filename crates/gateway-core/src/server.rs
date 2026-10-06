//! 网关 HTTP 服务：axum 路由 + 鉴权中间件 + 优雅停机。
//! 缝 1 的被测边界——测试进程内 `start()` 起真实监听。

use std::net::SocketAddr;

use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::HeaderName;
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::config::{AuthMode, GatewayConfig};
use crate::error::ApiError;

#[derive(Clone)]
pub struct AppState {
    pub config: GatewayConfig,
}

pub struct GatewayHandle {
    pub addr: SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl GatewayHandle {
    /// 优雅停机：等待进行中的请求收尾。
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        // 给 serve 任务一点收尾时间；测试退出时不必等待更久。
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// 在回环随机端口启动网关，返回句柄（缝 1 测试入口）。
pub async fn start(config: GatewayConfig) -> std::io::Result<GatewayHandle> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let state = AppState { config };
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
        .route("/health", get(health))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn list_models(State(_state): State<AppState>) -> impl IntoResponse {
    // T1.4 会聚合各 provider 目录；当前为空目录骨架。
    Json(json!({ "object": "list", "data": [] }))
}

/// 鉴权：Required 模式下接受 `Authorization: Bearer <key>` 或 `x-api-key: <key>`。
async fn auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: axum::extract::Request,
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
