use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// An error the client shows as is: `{"reason": "..."}` with a status.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub reason: String,
}

impl ApiError {
    pub fn new(status: StatusCode, reason: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason.into(),
        }
    }

    pub fn bad_request(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, reason)
    }

    pub fn unauthorized(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, reason)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "reason": self.reason }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!("database: {error}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong. Try again.",
        )
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
