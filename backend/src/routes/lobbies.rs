use crate::api_error::{ApiError, ErrorResponse};
use crate::database::{lobbies::LobbyError, nicknames};
use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};

use crate::{
    app_state::AppState,
    auth::AuthenticatedUser,
    models::lobby::{CreateLobbyRequest, CurrentLobby, JoinLobbyRequest},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(super::queue::router())
        .merge(super::game::lobby_router())
        .route("/", post(create_lobby))
        .route("/current", get(current_lobby))
        .route("/memberships/{membership_id}/events", get(events))
        .route("/memberships/{membership_id}", delete(leave_lobby))
        .route(
            "/memberships/{membership_id}/members/{user_id}",
            delete(remove_member),
        )
        .route("/join", post(join_lobby).layer(DefaultBodyLimit::max(1024)))
}

#[utoipa::path(
    delete, path = "/api/lobbies/memberships/{membership_id}", tag = "Lobbies",
    security(("bearer_auth" = [])),
    params(("membership_id" = String, Path, description = "Own membership generation: 32 lowercase hex characters")),
    responses(
        (status = 204, description = "Membership ended or already absent; owner departure closes the lobby for all. Empty body."),
        (status = 400, description = "Malformed membership ID or nonempty body", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 403, description = "Existing membership belongs to another account", body = ErrorResponse),
        (status = 500, description = "Departure failed without partial state", body = ErrorResponse),
        (status = 503, description = "Departure temporarily unavailable", body = ErrorResponse)
    )
)]
async fn leave_lobby(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<StatusCode, LobbyError> {
    let id = membership_path(path)?;
    axum::body::to_bytes(request.into_body(), 0)
        .await
        .map_err(|_| LobbyError::InvalidRequest)?;
    if let Some((lobby, revision)) = app
        .database
        .leave_lobby(user.id, &id, || (app.auth.clock)())
        .await?
    {
        app.lobby_updates.publish(lobby, revision);
        // A host leaving closes the lobby; its prompt images go right away.
        if app.database.sweep_game_images().await.is_err() {
            eprintln!("Game image cleanup temporarily unavailable");
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete, path = "/api/lobbies/memberships/{membership_id}/members/{user_id}", tag = "Lobbies",
    security(("bearer_auth" = [])),
    params(
        ("membership_id" = String, Path, description = "The host's own membership generation: 32 lowercase hex characters"),
        ("user_id" = String, Path, description = "Account ID of the member to remove, as in members[].user_id")
    ),
    responses(
        (status = 204, description = "Member removed, or already gone. Works like their leaving: their skip votes stop counting, game points stay (shown as left), a game phase waiting only on them closes, and their streams end with membership_ended reason removed. They can rejoin with the code. Empty body."),
        (status = 400, description = "Malformed IDs or nonempty body (invalid_request), or the target is the host (cannot_remove_host)", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 403, description = "Not the host (host_required), or the membership is not the caller's active one (membership_forbidden)", body = ErrorResponse),
        (status = 500, description = "Removal failed without partial state", body = ErrorResponse),
        (status = 503, description = "Removal temporarily unavailable", body = ErrorResponse)
    )
)]
async fn remove_member(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<(String, String)>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<StatusCode, LobbyError> {
    let (membership, target) = path.map_err(|_| LobbyError::InvalidRequest)?.0;
    let membership = membership_id(membership)?;
    let target: i64 = target
        .parse()
        .ok()
        .filter(|id: &i64| *id > 0 && target.bytes().all(|b| b.is_ascii_digit()))
        .ok_or(LobbyError::InvalidRequest)?;
    axum::body::to_bytes(request.into_body(), 0)
        .await
        .map_err(|_| LobbyError::InvalidRequest)?;
    if let crate::database::lobbies::Removal::Removed(lobby, revision) = app
        .database
        .remove_member(user.id, &membership, target, || (app.auth.clock)())
        .await?
    {
        app.lobby_updates.publish(lobby, revision);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn membership_path(
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
) -> Result<String, LobbyError> {
    membership_id(path.map_err(|_| LobbyError::InvalidRequest)?.0)
}

fn membership_id(id: String) -> Result<String, LobbyError> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(LobbyError::InvalidRequest);
    }
    Ok(id)
}

#[utoipa::path(
    get, path = "/api/lobbies/memberships/{membership_id}/events", tag = "Lobbies",
    security(("bearer_auth" = [])),
    params(("membership_id" = String, Path, description = "Caller membership generation: 32 lowercase hex characters")),
    responses(
        (status = 200, description = "SSE sync_required/lobby_changed revision hints; auth_expired or membership_ended left/closed/expired terminates access. Refetch current state after hints.", content_type = "text/event-stream", body = String),
        (status = 400, description = "Malformed membership ID", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 403, description = "Absent, ended or foreign membership", body = ErrorResponse),
        (status = 500, description = "Storage failure", body = ErrorResponse),
        (status = 503, description = "Storage temporarily unavailable", body = ErrorResponse)
    )
)]
async fn events(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
) -> Result<Response, LobbyError> {
    let id = membership_path(path)?;
    let receiver = app.lobby_updates.subscribe();
    app.database.require_lobby_account(user.id).await?;
    let bound = app
        .database
        .stream_membership(user.id, &id)
        .await?
        .filter(|s| s.closed_at.is_none() && s.expires_at.timestamp() > (app.auth.clock)())
        .ok_or(LobbyError::MembershipForbidden)?;
    if (app.auth.clock)() >= user.expires_at {
        return Err(LobbyError::Unauthorized);
    }
    let stream = crate::lobbies::updates::Connection::new(app, user, id, bound, receiver).stream();
    let mut response = axum::response::Sse::new(stream).into_response();
    response.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    Ok(response)
}

#[utoipa::path(
    get, path = "/api/lobbies/current", tag = "Lobbies",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Coherent current lobby or paired null membership_id and lobby", body = CurrentLobby),
        (status = 401, description = "Missing, invalid or expired token, or nonexistent account", body = ErrorResponse),
        (status = 500, description = "Current state could not be read", body = ErrorResponse),
        (status = 503, description = "Current state temporarily unavailable", body = ErrorResponse)
    )
)]
async fn current_lobby(
    State(app_state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<CurrentLobby>, LobbyError> {
    app_state
        .database
        .current_lobby(user.id, || (app_state.auth.clock)())
        .await
        .map(Json)
}

#[utoipa::path(
    post, path = "/api/lobbies/join", tag = "Lobbies",
    security(("bearer_auth" = [])), request_body = JoinLobbyRequest,
    responses(
        (status = 200, description = "Joined lobby, atomically leaving any previous guest membership, or same-lobby membership (renamed when a different nickname is sent; the host is never renamed this way)", body = CurrentLobby),
        (status = 400, description = "Invalid JSON, field, join code (invalid_join_code) or nickname (invalid_nickname)", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 404, description = "Unknown, closed or expired code", body = ErrorResponse),
        (status = 409, description = "Hosting another lobby", body = ErrorResponse),
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
                    "Expected a JSON object containing join_code and an optional nickname.",
                ),
            };
            return ApiError::new(status, code, message).into_response();
        }
    };
    let Some(code) = request.normalized_code() else {
        return LobbyError::InvalidJoinCode.into_response();
    };
    let nickname = match request.nickname.as_deref().map(nicknames::normalize) {
        None => None,
        Some(Some(name)) => Some(name),
        Some(None) => return LobbyError::InvalidNickname.into_response(),
    };
    match app_state
        .database
        .join_lobby_named(
            user.id,
            &code,
            nickname.as_deref(),
            || (app_state.auth.clock)(),
            || crate::models::lobby::MembershipId::random().map_err(|_| LobbyError::Unavailable),
        )
        .await
    {
        Ok((view, changed, departed)) => {
            if let Some((lobby, revision)) = departed {
                app_state.lobby_updates.publish(lobby, revision);
            }
            if changed {
                publish(&app_state, &view);
            }
            Json(view).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Creates a lobby with the authenticated Google account as owner and first member.
///
/// The lobby expires 24 hours after creation. The body is empty (the host keeps their
/// last nickname, or "Host") or `{"nickname": "..."}`. Guest accounts cannot host.
#[utoipa::path(
    post,
    path = "/api/lobbies",
    tag = "Lobbies",
    security(("bearer_auth" = [])),
    request_body(content = Option<CreateLobbyRequest>, description = "Optional: an empty body is also accepted"),
    responses(
        (status = 401, description = "Missing, invalid or expired bearer token", body = ErrorResponse),
        (status = 201, description = "Lobby and owner membership created", body = CurrentLobby),
        (status = 400, description = "Body is neither empty nor {\"nickname\"} (invalid_request), or the nickname breaks the rules (invalid_nickname)", body = ErrorResponse),
        (status = 403, description = "Guest accounts cannot host; sign in with Google (google_account_required)", body = ErrorResponse),
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
    let body = axum::body::to_bytes(request.into_body(), 1024)
        .await
        .map_err(|_| LobbyError::InvalidRequest)?;
    let nickname = if body.is_empty() {
        None
    } else {
        let request: CreateLobbyRequest =
            serde_json::from_slice(&body).map_err(|_| LobbyError::InvalidRequest)?;
        Some(nicknames::normalize(&request.nickname).ok_or(LobbyError::InvalidNickname)?)
    };
    let account = app_state
        .database
        .get_user(user.id)
        .await?
        .ok_or(LobbyError::Unauthorized)?;
    if account.is_guest() {
        return Err(LobbyError::GoogleAccountRequired);
    }
    let view = app_state
        .database
        .create_named_lobby(user.id, nickname.as_deref(), || (app_state.auth.clock)())
        .await?;
    publish(&app_state, &view);
    Ok((StatusCode::CREATED, Json(view)))
}

fn publish(app: &AppState, view: &CurrentLobby) {
    if let Some(lobby) = &view.lobby {
        app.lobby_updates.publish(
            lobby.id.clone(),
            lobby.revision.parse().expect("database revision"),
        );
    }
}

impl IntoResponse for LobbyError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::MembershipForbidden => (
                StatusCode::FORBIDDEN,
                "membership_forbidden",
                "Membership is not available.",
            ),
            Self::Unauthorized => {
                let mut response = crate::auth::AuthError::Unauthorized.into_response();
                response.headers_mut().insert(
                    axum::http::header::WWW_AUTHENTICATE,
                    axum::http::HeaderValue::from_static("Bearer"),
                );
                return response;
            }
            Self::HostRequired => (
                StatusCode::FORBIDDEN,
                "host_required",
                "Only the host can do this.",
            ),
            Self::CannotRemoveHost => (
                StatusCode::BAD_REQUEST,
                "cannot_remove_host",
                "The host can't be removed. End the session instead.",
            ),
            Self::GoogleAccountRequired => (
                StatusCode::FORBIDDEN,
                "google_account_required",
                "Sign in with Google to host a lobby. Guests can join with a code.",
            ),
            Self::InvalidNickname => (
                StatusCode::BAD_REQUEST,
                "invalid_nickname",
                "Nicknames are 1 to 24 characters, with no invisible or control characters.",
            ),
            Self::AlreadyInLobby => (
                StatusCode::CONFLICT,
                "already_in_lobby",
                "You already belong to an active lobby. If you are hosting, leave it to end the session before joining another.",
            ),
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Invalid request body or membership ID.",
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
                "Session codes are six letters or numbers, like A1B2C3.",
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
