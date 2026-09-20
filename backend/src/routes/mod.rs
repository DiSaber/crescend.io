mod lobbies;
mod youtube;

use axum::Router;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use crate::app_state::AppState;

#[derive(OpenApi)]
#[openapi(
    info(title = "Crescend.io API"),
    paths(lobbies::create_lobby, youtube::metadata),
    tags(
        (name = "Lobbies", description = "Lobby endpoints"),
        (name = "YouTube", description = "YouTube endpoints")
    )
)]
struct ApiDoc;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(Scalar::with_url("/scalar", ApiDoc::openapi()))
        .nest("/api/lobbies", lobbies::router())
        .nest("/api/youtube", youtube::router())
}
