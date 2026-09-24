pub mod browser;
mod config;
pub mod destination;
mod google;
mod jwt;
pub mod refresh;
mod transactions;

pub use config::AuthConfig;
pub use jwt::{AuthenticatedUser, JwtAuth, TOKEN_SECONDS};
pub use transactions::LOGIN_SECONDS;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

// AppState holds this in an Arc: requests share one login store and Google key cache.
pub struct Auth {
    pub config: AuthConfig,
    pub jwt: JwtAuth,
    pub google: google::Google,
    pub transactions: transactions::Transactions,
    pub clock: Clock,
}

impl Auth {
    pub fn new(config: AuthConfig) -> Result<Self, &'static str> {
        Self::with_clock(config, Arc::new(|| chrono::Utc::now().timestamp()))
    }

    pub fn with_clock(config: AuthConfig, clock: Clock) -> Result<Self, &'static str> {
        Ok(Self {
            jwt: JwtAuth::new(config.signing_secret.as_bytes(), clock.clone()),
            google: google::Google::new(&config)?,
            transactions: transactions::Transactions::new(4096),
            config,
            clock,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    BadRequest,
    Forbidden,
    Unauthorized,
    Unavailable,
    Internal,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: &'static str,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::BadRequest => (StatusCode::BAD_REQUEST, "Invalid authentication request."),
            Self::Forbidden => (StatusCode::FORBIDDEN, "Invalid cookie operation."),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Authentication failed."),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Authentication temporarily unavailable.",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Authentication could not be completed.",
            ),
        };
        (status, Json(ErrorResponse { error })).into_response()
    }
}

#[cfg(test)]
mod tests;
