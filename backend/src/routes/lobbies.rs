use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{TimeDelta, Utc};

use crate::{
    app_state::AppState,
    models::lobby::{Lobby, LobbyId},
};

const LOBBY_LIFETIME: TimeDelta = TimeDelta::minutes(1);

pub fn router() -> Router<AppState> {
    Router::new().route("/", post(create_lobby))
}

// TODO: Should this also return the initial owner/admin session token?
/// Creates a new lobby and returns its identifier.
///
/// The lobby expires one minute after creation. No request body is required.
#[utoipa::path(
    post,
    path = "/api/lobbies",
    tag = "Lobbies",
    responses(
        (status = 200, description = "Lobby created", body = LobbyId),
        (status = 500, description = "Database operation failed", body = String, content_type = "text/plain", example = "Something went wrong.")
    )
)]
async fn create_lobby(State(app_state): State<AppState>) -> Result<Json<LobbyId>, LobbyError> {
    // TODO: Retry until unique id?
    // probably fine to leave for now
    let lobby_id = LobbyId::new(rand::random());
    let created_at = Utc::now();
    let lobby = Lobby {
        id: lobby_id,
        created_at,
        expires_at: created_at + LOBBY_LIFETIME,
    };
    app_state.database.create_lobby(lobby).await?;

    Ok(Json(lobby_id))
}

// TODO: Probably a shared `AppError`
#[derive(Debug)]
pub enum LobbyError {
    Database(sqlx::Error),
}

impl From<sqlx::Error> for LobbyError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl IntoResponse for LobbyError {
    fn into_response(self) -> Response {
        match self {
            Self::Database(error) => {
                eprintln!("lobby database operation failed: {error}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong.").into_response()
            }
        }
    }
}
