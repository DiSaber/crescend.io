use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;

use crate::{
    app_state::AppState,
    models::lobby::{ClientLobby, JoinCode, LobbyId},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_lobby))
        .route("/{lobby_id}", get(get_lobby))
}

// TODO: Should this also return the initial owner/admin session token?
/// Creates a new lobby and returns its identifier.
async fn create_lobby(State(app_state): State<AppState>) -> Result<Json<LobbyId>, LobbyError> {
    // TODO: Retry until unique code?
    // probably fine to leave for now
    let join_code = JoinCode::new(rand::random());
    let lobby_id = app_state
        .database
        .create_lobby(join_code, Utc::now())
        .await?;

    Ok(Json(lobby_id))
}

async fn get_lobby(
    State(app_state): State<AppState>,
    Path(lobby_id): Path<LobbyId>,
) -> Result<Json<ClientLobby>, LobbyError> {
    let lobby = app_state
        .database
        .get_active_lobby(lobby_id, Utc::now())
        .await?
        .ok_or(LobbyError::NotFound)?;

    Ok(Json(lobby.into()))
}

// TODO: Probably a shared `AppError`
#[derive(Debug)]
pub enum LobbyError {
    NotFound,
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
            Self::NotFound => (StatusCode::NOT_FOUND, "Lobby not found.").into_response(),
            Self::Database(error) => {
                eprintln!("lobby database operation failed: {error}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong.").into_response()
            }
        }
    }
}
