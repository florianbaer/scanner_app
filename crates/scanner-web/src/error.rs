//! Maps core errors to HTTP responses with a JSON `{ "error": "…" }` body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use scanner_core::service::ServiceError;

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn not_found(what: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: format!("{what} not found"),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        let status = match e.downcast_ref::<ServiceError>() {
            Some(ServiceError::Busy) => StatusCode::CONFLICT,
            Some(ServiceError::Invalid(_)) => StatusCode::BAD_REQUEST,
            None => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            message: format!("{e:#}"),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        anyhow::Error::from(e).into()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: &self.message,
        };
        (self.status, Json(body)).into_response()
    }
}

#[derive(serde::Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

pub type ApiResult<T> = Result<T, ApiError>;
