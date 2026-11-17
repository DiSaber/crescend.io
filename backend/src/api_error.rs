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
    /// For `invalid_game_settings`: the settings key that was rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
    /// For `invalid_game_settings`: why that key was rejected, safe to show next to the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, error: &'static str) -> Self {
        Self {
            status,
            body: ErrorResponse {
                code,
                error,
                setting: None,
                detail: None,
            },
        }
    }
}

impl ApiError {
    /// Names the rejected settings key and a field-level message.
    pub fn with_setting(mut self, setting: String, detail: String) -> Self {
        self.body.setting = Some(setting);
        self.body.detail = Some(detail);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
