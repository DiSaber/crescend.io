use crate::api_error::{ApiError, ErrorResponse};
use crate::database::lobbies::LobbyError;
use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Request, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};

use crate::{
    app_state::AppState,
    auth::AuthenticatedUser,
    models::lobby::{CurrentLobby, JoinLobbyRequest},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_lobby))
        .route("/join", post(join_lobby).layer(DefaultBodyLimit::max(1024)))
}

#[utoipa::path(
    post, path = "/api/lobbies/join", tag = "Lobbies",
    security(("bearer_auth" = [])), request_body = JoinLobbyRequest,
    responses(
        (status = 200, description = "Joined lobby or unchanged same-lobby membership", body = CurrentLobby),
        (status = 400, description = "Invalid JSON, field or join code", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 404, description = "Unknown, closed or expired code", body = ErrorResponse),
        (status = 409, description = "Already in another lobby", body = ErrorResponse),
        (status = 413, description = "JSON body exceeds 1 KiB", body = ErrorResponse),
        (status = 415, description = "JSON Content-Type required", body = ErrorResponse),
        (status = 500, description = "Join failed without partial state", body = ErrorResponse),
        (status = 503, description = "Join temporarily unavailable", body = ErrorResponse)
    )
)]
async fn join_lobby(
    State(app_state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    payload: Result<Json<JoinLobbyRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(value) => value,
        Err(error) => {
            let (status, code, message) = match error.status() {
                StatusCode::PAYLOAD_TOO_LARGE => (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request_too_large",
                    "Join request exceeds 1 KiB.",
                ),
                StatusCode::UNSUPPORTED_MEDIA_TYPE => (
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "unsupported_media_type",
                    "Join requires JSON Content-Type.",
                ),
                _ => (
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Expected a JSON object containing only join_code.",
                ),
            };
            return ApiError::new(status, code, message).into_response();
        }
    };
    let Some(code) = request.normalized_code() else {
        return LobbyError::InvalidJoinCode.into_response();
    };
    match app_state
        .database
        .join_lobby(user.id, &code, || (app_state.auth.clock)())
        .await
    {
        Ok(view) => Json(view).into_response(),
        Err(error) => error.into_response(),
    }
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
                "Lobby operation is temporarily unavailable.",
            ),
            Self::Database(error) => {
                eprintln!("lobby database operation failed: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Lobby operation could not be completed.",
                )
            }
            Self::InvalidJoinCode => (
                StatusCode::BAD_REQUEST,
                "invalid_join_code",
                "Join code must contain six ASCII letters or digits.",
            ),
            Self::LobbyUnavailable => (
                StatusCode::NOT_FOUND,
                "lobby_unavailable",
                "Lobby code is invalid or unavailable.",
            ),
        };
        ApiError::new(status, code, message).into_response()
    }
}
