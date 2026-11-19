use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde_json::json;

/// Central error type for request handlers. Never leaks internal detail to
/// the caller: `Internal` logs the real error and returns a generic
/// message, matching the standing "never leak internal messages" rule.
#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    SignInRequired,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict(String),
    ServiceUnavailable(String),
    Internal,
}

impl AppError {
    fn status_and_message(&self) -> (StatusCode, String) {
        match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::SignInRequired => (StatusCode::UNAUTHORIZED, "sign in required".to_string()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.clone()),
            AppError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if matches!(self, AppError::SignInRequired) {
            return Redirect::to("/signin").into_response();
        }
        let (status, message) = self.status_and_message();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(?self, "request failed");
        }
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "database error");
        AppError::Internal
    }
}
