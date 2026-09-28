//! Shared public HTTP error contract. Domain errors select safe messages at their boundary.
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    /// Stable machine-readable identifier.
    pub code: &'static str,
    /// Safe human-readable explanation; never contains internal error details.
    pub error: &'static str,
}

pub struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, error: &'static str) -> Self {
        Self {
            status,
            body: ErrorResponse { code, error },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
