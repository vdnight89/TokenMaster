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
}

impl ApiError {
    fn parts(self) -> (StatusCode, String, String) {
        match self {
            ApiError::InvalidApiKey => (
                StatusCode::UNAUTHORIZED,
                "invalid_api_key".into(),
                "invalid or missing gateway key".into(),
            ),
            ApiError::ModelNotFound => (
                StatusCode::NOT_FOUND,
                "model_not_found".into(),
                "model not found: no route for this model name".into(),
            ),
            ApiError::Message(m) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error".into(),
                m,
            ),
            ApiError::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited".into(),
                self.to_string(),
            ),
            ApiError::Upstream { status, .. } => {
                let code = match status {
                    401 => "provider_credentials_rejected",
                    402 => "provider_payment_required",
                    403 => "provider_forbidden",
                    _ => "upstream_error",
                };
                let s = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                (s, code.to_string(), self.to_string())
            }
        }
    }
}

/// 错误体（与 HTTP 响应同一 JSON 形状），供单元断言与日志使用。
pub fn error_json(e: &ApiError) -> Value {
    let (_, code, message) = match e {
        ApiError::InvalidApiKey => (
            StatusCode::UNAUTHORIZED,
            "invalid_api_key".to_string(),
            "invalid or missing gateway key".to_string(),
        ),
        ApiError::ModelNotFound => (
            StatusCode::NOT_FOUND,
            "model_not_found".to_string(),
            "model not found: no route for this model name".to_string(),
        ),
        ApiError::Message(m) => (StatusCode::BAD_REQUEST, "invalid_request_error".into(), m.clone()),
        ApiError::RateLimited { .. } => (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited".into(),
            e.to_string(),
        ),
        ApiError::Upstream { status, .. } => {
            let code = match status {
                401 => "provider_credentials_rejected",
                402 => "provider_payment_required",
                403 => "provider_forbidden",
                _ => "upstream_error",
            };
            (
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY),
                code.to_string(),
                e.to_string(),
            )
        }
    };
    json!({ "error": { "message": message, "type": "invalid_request_error", "code": code } })
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        (
            status,
            Json(json!({
                "error": { "message": message, "type": "invalid_request_error", "code": code }
            })),
        )
            .into_response()
    }
}
