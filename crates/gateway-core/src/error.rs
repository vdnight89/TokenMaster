//! OpenAI 形状的 API 错误响应。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("invalid or missing gateway key")]
    InvalidApiKey,
    #[error("model not found: no route for this model name")]
    ModelNotFound,
    #[error("{0}")]
    Message(String),
    #[error("upstream rate limited, retry after {retry_after_secs:?}s: {msg}")]
    RateLimited { retry_after_secs: Option<u64>, msg: String },
    #[error("upstream error ({code}): {msg}")]
    Upstream { status: u16, code: String, msg: String },
    #[error("no available account for provider {provider}")]
    NoAvailableAccount { provider: String },
    #[error("all accounts failed after retries: {0}")]
    RetryExhausted(String),
}

impl ApiError {
    fn fields(&self) -> (StatusCode, String, String) {
        match self {
            ApiError::InvalidApiKey => (
                StatusCode::UNAUTHORIZED,
                "invalid_api_key".into(),
                self.to_string(),
            ),
            ApiError::ModelNotFound => (
                StatusCode::NOT_FOUND,
                "model_not_found".into(),
                self.to_string(),
            ),
            ApiError::Message(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error".into(),
                self.to_string(),
            ),
            ApiError::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited".into(),
                self.to_string(),
            ),
            ApiError::Upstream { status, code, .. } => (
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY),
                code.clone(),
                self.to_string(),
            ),
            ApiError::NoAvailableAccount { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "no_available_account".into(),
                self.to_string(),
            ),
            ApiError::RetryExhausted(_) => (
                StatusCode::BAD_GATEWAY,
                "retry_exhausted".into(),
                self.to_string(),
            ),
        }
    }
}

/// 错误体（与 HTTP 响应同一 JSON 形状），供单元断言与日志使用。
pub fn error_json(e: &ApiError) -> Value {
    let (_, code, message) = e.fields();
    json!({ "error": { "message": message, "type": "invalid_request_error", "code": code } })
}

/// 对外 HTTP 状态码（账本记录用）。
pub fn status_u16(e: &ApiError) -> u16 {
    e.fields().0.as_u16()
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.fields();
        (
            status,
            Json(json!({
                "error": { "message": message, "type": "invalid_request_error", "code": code }
            })),
        )
            .into_response()
    }
}
