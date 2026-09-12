mod lobbies;
mod youtube;

use axum::Router;

use crate::app_state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/api/lobbies", lobbies::router())
        .nest("/api/youtube", youtube::router())
}
