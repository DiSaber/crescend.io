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
const START_LIMIT: usize = 32 * 1024;
/// Multipart framing allowance on top of the image itself.
const UPLOAD_LIMIT: usize = crate::game::images::MAX_IMAGE_BYTES + 64 * 1024;

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
        .route("/memberships/{membership_id}/game/guesses", post(guess))
        .route(
            "/memberships/{membership_id}/game/images",
            get(list_images)
                .post(upload_image)
                .layer(axum::extract::DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route(
            "/memberships/{membership_id}/game/images/{image_id}",
            get(image).delete(delete_image),
        )
}

pub fn modes_router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_modes).post(save_mode))
        .route("/{mode_id}", axum::routing::delete(delete_mode))
}

fn mode_view(
    id: String,
    description: &str,
    preset: bool,
    definition: modes::Definition,
    default_prompts: Vec<GamePrompt>,
) -> GameModeView {
    GameModeView {
        id,
        settings: modes::schema(&definition),
        name: definition.name,
        description: description.into(),
        preset,
        rules: definition.rules,
        default_prompts,
    }
}

#[utoipa::path(post, path = "/api/game-modes", tag = "Games",
    security(("bearer_auth" = [])), request_body = SaveModeRequest,
    responses((status = 201, body = GameModeView), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 409, body = ErrorResponse), (status = 413, body = ErrorResponse), (status = 415, body = ErrorResponse),
        (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn save_mode(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<(StatusCode, Json<GameModeView>), GameError> {
    let payload: SaveModeRequest = json(request, START_LIMIT).await?;
    let definition = crate::game::customize(
        crate::game::custom_definition(&payload.name, payload.rules)?,
        &payload.settings,
    )?;
    let prompts = crate::game::saved_prompts(definition.rules.prompt, &payload.prompts)?;
    let saved = app
        .database
        .save_game_mode(&user, &definition, &prompts, || (app.auth.clock)())
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(mode_view(
            saved.id,
            SAVED,
            false,
            saved.definition,
            saved.prompts,
        )),
    ))
}

const SAVED: &str = "Your saved mode.";

#[utoipa::path(delete, path = "/api/game-modes/{mode_id}", tag = "Games",
    security(("bearer_auth" = [])), params(("mode_id" = String, Path, description = "A saved mode's 32-hex ID")),
    responses((status = 204, description = "Deleted, or not one of the caller's saved modes"), (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn delete_mode(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<StatusCode, GameError> {
    let id = membership(path)?;
    bodyless(request).await?;
    app.database
        .delete_game_mode(&user, &id, || (app.auth.clock)())
        .await?;
    Ok(StatusCode::NO_CONTENT)
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
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<GameModesResponse>, GameError> {
    bodyless(request).await?;
    let saved = app
        .database
        .saved_game_modes(&user, || (app.auth.clock)())
        .await?;
    let presets = modes::PRESETS.iter().map(|preset| {
        mode_view(
            preset.id.into(),
            preset.description,
            true,
            (preset.definition)(),
            Vec::new(),
        )
    });
    let saved = saved
        .into_iter()
        .map(|mode| mode_view(mode.id, SAVED, false, mode.definition, mode.prompts));
    Ok(Json(GameModesResponse {
        modes: presets.chain(saved).collect(),
    }))
}

#[utoipa::path(get, path = "/api/lobbies/memberships/{membership_id}/game", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path),
        ("view" = Option<String>, Query, description = "`board` for the shared TV view: the guest-visible snapshot even on the host's account")),
    responses((status = 200, body = GameSnapshot), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn read(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    request: Request,
) -> Result<Json<GameSnapshot>, GameError> {
    let id = membership(path)?;
    let board = match query.as_deref() {
        None | Some("") => false,
        Some("view=board") => true,
        Some(_) => return Err(GameError::InvalidRequest),
    };
    bodyless(request).await?;
    app.database
        .game_view(&user, &id, board, || (app.auth.clock)())
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
    let payload: StartGameRequest = json(request, START_LIMIT).await?;
    // A 32-hex mode ID names one of the host's saved modes; anything else a preset.
    let saved = match payload.mode_id.as_deref().filter(|id| valid_id(id)) {
        Some(mode) => Some(app.database.saved_game_mode(&user, mode).await?),
        None => None,
    };
    let (defaults, base) = match saved {
        Some(mode) => (mode.prompts, Some(mode.definition)),
        None => (Vec::new(), None),
    };
    let (mode_id, definition) = crate::game::resolve(&payload, base)?;
    let requested = if payload.prompts.is_empty() {
        &defaults
    } else {
        &payload.prompts
    };
    let prompts = crate::game::prompts(definition.rules.prompt, requested)?;
    let game = app
        .database
        .start_game(&user, &id, &mode_id, &definition, &prompts, || {
            (app.auth.clock)()
        })
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

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/game/guesses", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = GameGuessRequest,
    responses((status = 200, body = GameGuessResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 413, body = ErrorResponse),
        (status = 415, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn guess(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameGuessResponse>, GameError> {
    let id = membership(path)?;
    let payload: GameGuessRequest = json(request, BODY_LIMIT).await?;
    let text = payload.text.trim();
    if !(1..=crate::game::answer::MAX_GUESS_CHARS).contains(&text.chars().count()) {
        return Err(GameError::InvalidRequest);
    }
    let response = app
        .database
        .guess_game(&user, &id, text, || (app.auth.clock)())
        .await?;
    publish(&app, response.changed, &response.game);
    Ok(Json(response))
}

type ImagePath = Result<Path<(String, String)>, axum::extract::rejection::PathRejection>;

fn image_path(path: ImagePath) -> Result<(String, String), GameError> {
    let Path((membership, image)) = path.map_err(|_| GameError::InvalidRequest)?;
    if !valid_id(&membership) || !valid_id(&image) {
        return Err(GameError::InvalidRequest);
    }
    Ok((membership, image))
}

/// `multipart/form-data` with one file field named `image`.
#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct ImageUpload {
    #[schema(value_type = String, format = Binary)]
    image: Vec<u8>,
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/game/images", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)),
    request_body(content = ImageUpload, content_type = "multipart/form-data"),
    responses((status = 200, body = GameImage), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 413, body = ErrorResponse),
        (status = 415, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 500, body = ErrorResponse),
        (status = 503, body = ErrorResponse)))]
pub(super) async fn upload_image(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    multipart: Result<axum::extract::Multipart, axum::extract::multipart::MultipartRejection>,
) -> Result<Json<GameImage>, GameError> {
    use crate::game::images::{MAX_IMAGE_BYTES, process};
    let id = membership(path)?;
    // Authorize before reading or decoding anything.
    app.database
        .require_game_host(&user, &id, || (app.auth.clock)())
        .await?;
    let mut multipart = multipart.map_err(|_| GameError::MediaType)?;
    let mut upload: Option<Vec<u8>> = None;
    while let Some(mut field) = multipart.next_field().await.map_err(multipart_error)? {
        if field.name() != Some("image") || upload.is_some() {
            return Err(GameError::InvalidRequest);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
            if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
                return Err(GameError::Image(crate::game::images::ImageError::TooLarge));
            }
            bytes.extend_from_slice(&chunk);
        }
        upload = Some(bytes);
    }
    let bytes = upload.ok_or(GameError::InvalidRequest)?;
    let processed = tokio::task::spawn_blocking(move || process(&bytes))
        .await
        .map_err(|_| GameError::Unavailable)?
        .map_err(GameError::Image)?;
    app.database
        .store_game_image(&user, &id, &processed, || (app.auth.clock)())
        .await
        .map(Json)
}

fn multipart_error(error: axum::extract::multipart::MultipartError) -> GameError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        GameError::Image(crate::game::images::ImageError::TooLarge)
    } else {
        GameError::InvalidRequest
    }
}

#[utoipa::path(get, path = "/api/lobbies/memberships/{membership_id}/game/images", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)),
    responses((status = 200, body = GameImagesResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn list_images(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: MembershipPath,
    request: Request,
) -> Result<Json<GameImagesResponse>, GameError> {
    let id = membership(path)?;
    bodyless(request).await?;
    app.database
        .list_game_images(&user, &id, || (app.auth.clock)())
        .await
        .map(Json)
}

#[utoipa::path(get, path = "/api/lobbies/memberships/{membership_id}/game/images/{image_id}", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path), ("image_id" = String, Path)),
    responses((status = 200, description = "The re-encoded image (image/jpeg or image/png)", content_type = "image/*", body = Vec<u8>),
        (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn image(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: ImagePath,
    request: Request,
) -> Result<Response, GameError> {
    let (membership, image) = image_path(path)?;
    bodyless(request).await?;
    let (content_type, file) = app
        .database
        .game_image(&user, &membership, &image, || (app.auth.clock)())
        .await?;
    let bytes = tokio::fs::read(&file).await.map_err(|error| {
        eprintln!("game image missing from storage: {error}");
        GameError::ImageUnavailable
    })?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        bytes,
    )
        .into_response())
}

#[utoipa::path(delete, path = "/api/lobbies/memberships/{membership_id}/game/images/{image_id}", tag = "Games",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path), ("image_id" = String, Path)),
    responses((status = 204, description = "Deleted or already absent"), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse),
        (status = 503, body = ErrorResponse)))]
pub(super) async fn delete_image(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: ImagePath,
    request: Request,
) -> Result<StatusCode, GameError> {
    let (membership, image) = image_path(path)?;
    bodyless(request).await?;
    app.database
        .delete_game_image(&user, &membership, &image, || (app.auth.clock)())
        .await?;
    Ok(StatusCode::NO_CONTENT)
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
            Self::InvalidSettings { .. } => (
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
            Self::SongAlreadyPicked => (
                409,
                "song_already_picked",
                "Someone already picked that song this round. Pick another.",
            ),
            Self::NotEnoughPlayers(message) => (409, "not_enough_players", message),
            Self::InvalidModeName(message) => (400, "invalid_mode_name", message),
            Self::OwnPair => (
                409,
                "own_pair",
                "Your song is in this pair, so you sit this one out.",
            ),
            Self::DuplicateSong => (
                409,
                "song_already_proposed",
                "That song is already in this game; pick another.",
            ),
            Self::HostCannotGuess => (
                403,
                "host_cannot_guess",
                "The host's screen shows the song, so the host does not guess.",
            ),
            Self::OwnSong => (
                409,
                "own_song",
                "This is your song; you score when others guess it.",
            ),
            Self::GuessLimit => (409, "guess_limit_reached", "No guesses left this round."),
            Self::InvalidPrompts => (
                400,
                "invalid_game_prompts",
                "Prompts do not fit this mode: check the count, text and images.",
            ),
            Self::ModeLimit => (
                409,
                "mode_limit_reached",
                "You have saved the most modes allowed; delete one first.",
            ),
            Self::ImageUnavailable => (404, "game_image_unavailable", "Image unavailable."),
            Self::ImageInUse => (
                409,
                "image_in_use",
                "The running game still uses this image.",
            ),
            Self::ImageLimit => (
                409,
                "image_limit_reached",
                "This lobby already holds the most images allowed.",
            ),
            Self::Image(crate::game::images::ImageError::TooLarge) => {
                (413, "image_too_large", "Images must be 5 MB or smaller.")
            }
            Self::Image(crate::game::images::ImageError::Type) => (
                415,
                "unsupported_image_type",
                "Upload a JPEG, PNG or WebP image.",
            ),
            Self::Image(crate::game::images::ImageError::Dimensions) => (
                422,
                "image_dimensions_exceeded",
                "Images must be at most 4096 pixels on each side.",
            ),
            Self::Image(crate::game::images::ImageError::Invalid) => {
                (422, "invalid_image", "The image could not be read.")
            }
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
        let mut error = ApiError::new(StatusCode::from_u16(status).unwrap(), code, message);
        if let Self::InvalidSettings { setting, detail } = self {
            error = error.with_setting(setting, detail);
        }
        let mut response = error.into_response();
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
