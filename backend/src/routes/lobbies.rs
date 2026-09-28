use crate::api_error::{ApiError, ErrorResponse};
use crate::database::lobbies::LobbyError;
use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};

use crate::{app_state::AppState, auth::AuthenticatedUser, models::lobby::CurrentLobby};

pub fn router() -> Router<AppState> {
    Router::new().route("/", post(create_lobby))
}

/// Creates a lobby with the authenticated account as owner and first member.
///
/// The lobby expires 24 hours after creation. The request body must be empty.
#[utoipa::path(
    post,
    path = "/api/lobbies",
    tag = "Lobbies",
    security(("bearer_auth" = [])),
    responses(
        (status = 401, description = "Missing, invalid or expired bearer token", body = ErrorResponse),
        (status = 201, description = "Lobby and owner membership created", body = CurrentLobby),
        (status = 400, description = "Request body must be empty", body = ErrorResponse),
        (status = 409, description = "Account already belongs to an active lobby", body = ErrorResponse),
        (status = 500, description = "Creation failed without partial state", body = ErrorResponse),
        (status = 503, description = "Creation temporarily unavailable", body = ErrorResponse)
    )
)]
async fn create_lobby(
    State(app_state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<(StatusCode, Json<CurrentLobby>), LobbyError> {
    // A zero-byte limit rejects nonempty/chunked bodies without buffering them.
    axum::body::to_bytes(request.into_body(), 0)
        .await
        .map_err(|_| LobbyError::InvalidRequest)?;
    let view = app_state
        .database
        .create_owned_lobby(user.id, || (app_state.auth.clock)())
        .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

impl IntoResponse for LobbyError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => {
                let mut response = crate::auth::AuthError::Unauthorized.into_response();
                response.headers_mut().insert(
                    axum::http::header::WWW_AUTHENTICATE,
                    axum::http::HeaderValue::from_static("Bearer"),
                );
                return response;
            }
            Self::AlreadyInLobby => (
                StatusCode::CONFLICT,
                "already_in_lobby",
                "You already belong to an active lobby.",
            ),
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "This operation requires an empty request body.",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "Lobby creation is temporarily unavailable.",
            ),
            Self::Database(error) => {
                eprintln!("lobby database operation failed: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Lobby creation could not be completed.",
                )
            }
        };
        ApiError::new(status, code, message).into_response()
    }
}
