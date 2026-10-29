//! Authenticated catalog search.
use crate::{
    api_error::{ApiError, ErrorResponse},
    app_state::AppState,
    auth::AuthenticatedUser,
    models::song::{SongSearchQuery, SongSearchResponse, YouTubeSearchStatus},
    songs::{SongSearch, SongsError},
};
use axum::{
    Extension, Json, Router,
    extract::{Query, State, rejection::QueryRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/search", get(search))
}

/// Search the shared song catalog by title or artist.
///
/// Catalog-only: starter songs plus every song accepted into any queue. Results carry a
/// watch URL to submit through the queue API exactly like a pasted link.
#[utoipa::path(
    get, path = "/api/songs/search", tag = "Songs",
    security(("bearer_auth" = [])),
    params(SongSearchQuery),
    responses(
        (status = 200, description = "Matching songs, best first; empty when nothing matches", body = SongSearchResponse),
        (status = 400, description = "Missing or empty `q`, `q` over 100 characters, or `limit` outside 1-25", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = ErrorResponse),
        (status = 500, description = "Search failed", body = ErrorResponse),
        (status = 503, description = "Search temporarily unavailable", body = ErrorResponse)
    )
)]
pub(super) async fn search(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    query: Result<Query<SongSearchQuery>, QueryRejection>,
) -> Result<Json<SongSearchResponse>, SongsError> {
    let Query(query) = query.map_err(|_| SongsError::InvalidQuery)?;
    if (app.auth.clock)() >= user.expires_at {
        return Err(SongsError::Unauthorized);
    }
    let search = SongSearch::parse(&query.q, query.limit)?;
    let results = app.database.search_songs(user.id, &search).await?;
    // Seam for #41 part 2: when few results come back, search YouTube here and say so.
    Ok(Json(SongSearchResponse {
        results,
        youtube: YouTubeSearchStatus::NotConfigured,
    }))
}

impl IntoResponse for SongsError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => (401, "unauthorized", "Authentication required."),
            Self::InvalidQuery => (
                400,
                "invalid_search",
                "Search needs q (1-100 characters) and an optional limit from 1 to 25.",
            ),
            Self::Unavailable => (
                503,
                "temporarily_unavailable",
                "Search temporarily unavailable.",
            ),
            Self::Database(error) => {
                eprintln!("song search failure: {error}");
                (500, "internal_error", "Search failed.")
            }
        };
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
