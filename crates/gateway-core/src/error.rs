//! OpenAI 形状的 API 错误响应。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("invalid or missing gateway key")]
    InvalidApiKey,
    #[error("{0}")]
    Message(&'static str),
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str, String) {
        match self {
            ApiError::InvalidApiKey => (
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
                self.to_string(),
            ),
            ApiError::Message(m) => (StatusCode::BAD_REQUEST, "invalid_request_error", m.to_string()),
        }
    }
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
