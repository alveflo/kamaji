//! HTTP error mapping. Domain/db failures become `ApiError`, which renders a
//! JSON body `{ "error": "...", "kind": "..." }` with the matching status.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// An error surfaced to an HTTP client.
pub enum ApiError {
    /// The requested entity does not exist → 404.
    NotFound,
    /// The request was malformed or violated a precondition → 400.
    BadRequest(String),
    /// An unexpected internal failure → 500 (details logged, not leaked).
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, kind, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            ApiError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message, "kind": kind }))).into_response()
    }
}
