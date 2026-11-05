//! Game mode catalog and membership-bound game handlers.
use crate::{
    api_error::{ApiError, ErrorResponse},
    app_state::AppState,
    auth::AuthenticatedUser,
    game::{GameError, modes},
    models::{
        game::*,
        lobby::LobbyId,
        queue::{SubmitRequest, valid_id},
    },
};
use axum::{
    Extension, Json, Router,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::de::DeserializeOwned;

/// JSON bodies for game requests are small; start requests grow with prompts.
const BODY_LIMIT: usize = 4096;

pub fn lobby_router() -> Router<AppState> {
    Router::new()
        .route(
            "/memberships/{membership_id}/game",
            get(read).post(start).delete(stop),
        )
        .route("/memberships/{membership_id}/game/advance", post(advance))
        .route(
            "/memberships/{membership_id}/game/submissions",
            post(submit),
        )
        .route("/memberships/{membership_id}/game/votes", post(vote))
}

pub fn modes_router() -> Router<AppState> {
    Router::new().route("/", get(list_modes))
}

type MembershipPath = Result<Path<String>, axum::extract::rejection::PathRejection>;

fn membership(path: MembershipPath) -> Result<String, GameError> {
    let Path(id) = path.map_err(|_| GameError::InvalidRequest)?;
    valid_id(&id).then_some(id).ok_or(GameError::InvalidRequest)
}

#[utoipa::path(get, path = "/api/game-modes", tag = "Games",
    security(("bearer_auth" = [])),
    responses((status = 200, body = GameModesResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn list_modes(
    Extension(_user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<GameModesResponse>, GameError> {
    bodyless(request).await?;
    Ok(Json(GameModesResponse {
        modes: modes::PRESETS
            .iter()
            .map(|preset| {
                let definition = (preset.definition)();
                GameModeView {
                    id: preset.id.into(),
                    settings: modes::schema(&definition),
                    name: definition.name,
                    description: preset.description.into(),
                    preset: true,
                    rules: definition.rules,
                }
            })
            .collect(),
    }))
}

#[utoipa::path(get, path = "/api/lobbies/memberships/{membership_id}/game", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)),
    responses((status = 200, body = GameSnapshot), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn read(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameSnapshot>, GameError> {
    let id = membership(path)?;
    bodyless(request).await?;
    app.database
        .game_view(&user, &id, || (app.auth.clock)())
        .await
        .map(Json)
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/game", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = StartGameRequest,
    responses((status = 200, body = GameMutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse),
        (status = 413, body = ErrorResponse), (status = 415, body = ErrorResponse), (status = 500, body = ErrorResponse),
        (status = 503, body = ErrorResponse)))]
pub(super) async fn start(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameMutationResponse>, GameError> {
    let id = membership(path)?;
    let payload: StartGameRequest = json(request, BODY_LIMIT).await?;
    let (mode_id, definition) = crate::game::resolve(&payload)?;
    let game = app
        .database
        .start_game(&user, &id, &mode_id, &definition, || (app.auth.clock)())
        .await?;
    publish(&app, true, &game);
    Ok(Json(GameMutationResponse {
        changed: true,
        game,
    }))
}

#[utoipa::path(delete, path = "/api/lobbies/memberships/{membership_id}/game", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)),
    responses((status = 200, body = GameMutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn stop(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameMutationResponse>, GameError> {
    let id = membership(path)?;
    bodyless(request).await?;
    let response = app
        .database
        .stop_game(&user, &id, || (app.auth.clock)())
        .await?;
    publish(&app, response.changed, &response.game);
    Ok(Json(response))
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/game/advance", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = GameAdvanceRequest,
    responses((status = 200, body = GameMutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 413, body = ErrorResponse),
        (status = 415, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn advance(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameMutationResponse>, GameError> {
    let id = membership(path)?;
    let payload: GameAdvanceRequest = json(request, BODY_LIMIT).await?;
    let response = app
        .database
        .advance_game(&user, &id, &payload, || (app.auth.clock)())
        .await?;
    publish(&app, response.changed, &response.game);
    Ok(Json(response))
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/game/submissions", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = SubmitRequest,
    responses((status = 200, body = GameSubmissionResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 413, body = ErrorResponse),
        (status = 415, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn submit(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameSubmissionResponse>, GameError> {
    let id = membership(path)?;
    let payload: SubmitRequest = json(request, BODY_LIMIT).await?;
    if !valid_id(&payload.request_id) {
        return Err(GameError::InvalidRequest);
    }
    let response = crate::game::submit(&app, &user, &id, payload).await?;
    publish(&app, response.changed, &response.game);
    Ok(Json(response))
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/game/votes", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = GameVoteRequest,
    responses((status = 200, body = GameMutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse),
        (status = 413, body = ErrorResponse), (status = 415, body = ErrorResponse), (status = 500, body = ErrorResponse),
        (status = 503, body = ErrorResponse)))]
pub(super) async fn vote(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameMutationResponse>, GameError> {
    let id = membership(path)?;
    let payload: GameVoteRequest = json(request, BODY_LIMIT).await?;
    if !valid_id(&payload.submission_id) {
        return Err(GameError::InvalidRequest);
    }
    let response = app
        .database
        .vote_game(&user, &id, &payload.submission_id, || (app.auth.clock)())
        .await?;
    publish(&app, response.changed, &response.game);
    Ok(Json(response))
}

/// Publishes on the membership event stream exactly like queue commits.
fn publish(app: &AppState, changed: bool, view: &GameSnapshot) {
    if changed {
        let bytes = base16ct::lower::decode_vec(&view.lobby_id).expect("stored lobby ID");
        app.lobby_updates.publish(
            LobbyId::from_bytes(bytes.try_into().expect("128-bit lobby ID")),
            view.revision.value(),
        );
    }
}

pub(crate) async fn json<T: DeserializeOwned>(
    request: Request,
    limit: usize,
) -> Result<T, GameError> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime = content_type.split(';').next().unwrap_or("").trim();
    if !(mime.eq_ignore_ascii_case("application/json")
        || (mime.starts_with("application/") && mime.ends_with("+json")))
    {
        return Err(GameError::MediaType);
    }
    let bytes = axum::body::to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| GameError::TooLarge)?;
    serde_json::from_slice(&bytes).map_err(|_| GameError::InvalidRequest)
}

async fn bodyless(request: Request) -> Result<(), GameError> {
    axum::body::to_bytes(request.into_body(), 0)
        .await
        .map_err(|_| GameError::InvalidRequest)?;
    Ok(())
}

impl GameError {
    fn parts(&self) -> (u16, &'static str, &'static str) {
        match self {
            Self::Unauthorized => (401, "unauthorized", "Authentication required."),
            Self::MembershipForbidden => {
                (403, "membership_forbidden", "Active membership required.")
            }
            Self::HostRequired => (403, "host_required", "Host permission required."),
            Self::InvalidRequest => (400, "invalid_request", "Invalid game request."),
            Self::TooLarge => (413, "request_too_large", "Game request is too large."),
            Self::MediaType => (415, "unsupported_media_type", "JSON content type required."),
            Self::ModeUnavailable => (404, "game_mode_unavailable", "Game mode unavailable."),
            Self::InvalidRules => (
                400,
                "invalid_game_rules",
                "That combination of rules cannot run together.",
            ),
            Self::InvalidSettings => (
                400,
                "invalid_game_settings",
                "Settings must match the mode's settings schema.",
            ),
            Self::GameRunning => (
                409,
                "game_running",
                "A game is already running; end it first.",
            ),
            Self::NotRunning => (409, "game_not_running", "No game is running."),
            Self::PhaseClosed => (
                409,
                "game_phase_closed",
                "This action is not open in the current phase.",
            ),
            Self::PhaseChanged => (
                409,
                "game_phase_changed",
                "The phase changed; refetch the game.",
            ),
            Self::SongChanged => (
                409,
                "game_song_changed",
                "That song is no longer playing; refetch the game.",
            ),
            Self::SubmissionLimit => (
                409,
                "submission_limit_reached",
                "You have submitted all your songs for this round.",
            ),
            Self::SubmissionUnavailable => (
                404,
                "game_submission_unavailable",
                "That song is not on the current ballot.",
            ),
            Self::SelfVote => (
                409,
                "self_vote_forbidden",
                "You cannot vote for your own song.",
            ),
            Self::VoteLimit => (409, "vote_limit_reached", "You have used all your votes."),
            Self::IdempotencyConflict => (
                409,
                "idempotency_conflict",
                "Retry identity has different content.",
            ),
            Self::InvalidUrl => (
                422,
                "invalid_url",
                "A supported YouTube video URL is required.",
            ),
            Self::Upstream => (
                502,
                "upstream_unavailable",
                "Metadata service could not be reached.",
            ),
            Self::Metadata => (502, "metadata_unavailable", "Video metadata unavailable."),
            Self::Unavailable => (
                503,
                "temporarily_unavailable",
                "Game temporarily unavailable.",
            ),
            Self::Database(error) => {
                eprintln!("game storage failure: {error}");
                (500, "internal_error", "Game operation failed.")
            }
        }
    }
}

impl IntoResponse for GameError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        let mut response =
            ApiError::new(StatusCode::from_u16(status).unwrap(), code, message).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        if status == 401 {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
        }
        response
    }
}
