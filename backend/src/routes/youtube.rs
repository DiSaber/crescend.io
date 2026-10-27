use crate::api_error::{ApiError, ErrorResponse};
use crate::{
    app_state::AppState,
    youtube::{MetadataError, YouTubeVideo},
};
use axum::extract::State;
use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};

use crate::models::youtube::{MetadataRequest, VideoMetadata};

pub fn router() -> Router<crate::app_state::AppState> {
    Router::new().route("/metadata", post(metadata))
}

/// Fetch metadata for a YouTube video.
///
/// Normalizes supported video links and retrieves title, author, and thumbnail
/// information from YouTube oEmbed. Playlist and non-YouTube URLs are rejected.
#[utoipa::path(
    post,
    path = "/api/youtube/metadata",
    tag = "YouTube",
    request_body = MetadataRequest,
    responses(
        (status = 200, description = "Video metadata retrieved", body = VideoMetadata),
        (status = 400, description = "Malformed JSON request body", body = ErrorResponse),
        (status = 413, description = "Request body exceeds the size limit", body = ErrorResponse),
        (status = 415, description = "Missing or unsupported JSON content type", body = ErrorResponse),
        (status = 422, description = "Invalid YouTube video URL or JSON does not match the request schema", body = ErrorResponse),
        (status = 502, description = "YouTube metadata service could not be reached or metadata is unavailable", body = ErrorResponse)
    )
)]
async fn metadata(
    State(app): State<AppState>,
    request: Result<Json<MetadataRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<VideoMetadata>, YoutubeError> {
    let Json(request) = request.map_err(YoutubeError::Json)?;
    let video = YouTubeVideo::parse(&request.url).map_err(|_| YoutubeError::InvalidUrl)?;
    app.metadata
        .lookup(&video)
        .await
        .map(Json)
        .map_err(|error| match error {
            MetadataError::InvalidUrl => YoutubeError::InvalidUrl,
            MetadataError::Service => YoutubeError::Service,
            MetadataError::Unavailable => YoutubeError::Unavailable,
        })
}

#[derive(Debug)]
enum YoutubeError {
    Json(axum::extract::rejection::JsonRejection),
    InvalidUrl,
    Service,
    Unavailable,
}

impl IntoResponse for YoutubeError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Json(rejection) => match rejection.status() {
                StatusCode::PAYLOAD_TOO_LARGE => (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "payload_too_large",
                    "Request body exceeds the size limit.",
                ),
                StatusCode::UNSUPPORTED_MEDIA_TYPE => (
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "unsupported_media_type",
                    "A JSON content type is required.",
                ),
                StatusCode::UNPROCESSABLE_ENTITY => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_request",
                    "JSON does not match the request schema.",
                ),
                _ => (
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Malformed JSON request body.",
                ),
            },
            Self::InvalidUrl => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_url",
                "The URL must be a valid YouTube video link.",
            ),
            Self::Service => (
                StatusCode::BAD_GATEWAY,
                "upstream_unavailable",
                "The YouTube metadata service could not be reached.",
            ),
            Self::Unavailable => (
                StatusCode::BAD_GATEWAY,
                "metadata_unavailable",
                "YouTube metadata is unavailable for this video.",
            ),
        };
        ApiError::new(status, code, message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::YouTubeVideo;

    fn test_state() -> crate::app_state::AppState {
        let config = crate::auth::AuthConfig::load(|key| {
            Some(
                match key {
                    "GOOGLE_CLIENT_ID" => "test-client",
                    "GOOGLE_CLIENT_SECRET" => "test-secret",
                    "GOOGLE_REDIRECT_URI" => "http://localhost:3000/api/auth/google/callback",
                    "JWT_SIGNING_SECRET" => "test-only-signing-secret-at-least-32-bytes",
                    _ => return None,
                }
                .into(),
            )
        })
        .unwrap();
        crate::app_state::AppState {
            database: crate::database::Database {
                db_pool: sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_lazy("sqlite::memory:")
                    .unwrap(),
            },
            auth: std::sync::Arc::new(crate::auth::Auth::new(config).unwrap()),
            lobby_updates: Default::default(),
            metadata: crate::youtube::MetadataClient::new().unwrap(),
        }
    }

    #[tokio::test]
    async fn request_rejections_use_shared_json_errors() {
        use axum::{extract::FromRequest, response::IntoResponse};
        for (body, content_type, status, code) in [
            ("{", Some("application/json"), 400, "invalid_request"),
            ("{}", Some("application/json"), 422, "invalid_request"),
            ("{}", None, 415, "unsupported_media_type"),
            (
                r#"{"url":"https://example.com"}"#,
                Some("application/json"),
                422,
                "invalid_url",
            ),
        ] {
            let mut request = axum::http::Request::builder();
            if let Some(content_type) = content_type {
                request = request.header("content-type", content_type);
            }
            let request = request.body(axum::body::Body::from(body)).unwrap();
            let extracted =
                axum::Json::<crate::models::youtube::MetadataRequest>::from_request(request, &())
                    .await;
            let response = super::metadata(axum::extract::State(test_state()), extracted)
                .await
                .into_response();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(response.headers()["content-type"], "application/json");
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["code"], code);
            assert!(value["error"].is_string());
        }
    }

    #[test]
    fn normalizes_supported_video_links() {
        let video = YouTubeVideo::parse("https://youtu.be/dQw4w9WgXcQ?t=42").unwrap();
        assert_eq!(video.id, "dQw4w9WgXcQ");
        assert_eq!(
            video.watch_url.as_str(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert!(video.embed_url.contains("/embed/dQw4w9WgXcQ?"));
    }

    #[test]
    fn rejects_non_video_links_and_malformed_ids() {
        assert!(YouTubeVideo::parse("https://www.youtube.com/playlist?list=abc").is_err());
        assert!(YouTubeVideo::parse("https://example.com/watch?v=dQw4w9WgXcQ").is_err());
        assert!(YouTubeVideo::parse("https://youtu.be/short").is_err());
        assert!(YouTubeVideo::parse("javascript:alert(1)").is_err());
    }
}
