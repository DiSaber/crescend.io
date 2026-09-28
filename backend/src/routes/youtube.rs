use crate::api_error::{ApiError, ErrorResponse};
use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use url::Url;

use crate::models::youtube::{MetadataRequest, VideoMetadata};

const OEMBED_ENDPOINT: &str = "https://www.youtube.com/oembed";

pub fn router() -> Router<crate::app_state::AppState> {
    Router::new().route("/metadata", post(metadata))
}

#[derive(Debug, Deserialize)]
struct OEmbedResponse {
    title: String,
    author_name: String,
    author_url: String,
    thumbnail_url: String,
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
    request: Result<Json<MetadataRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<VideoMetadata>, YoutubeError> {
    let Json(request) = request.map_err(YoutubeError::Json)?;
    let video = YouTubeVideo::parse(&request.url).ok_or(YoutubeError::InvalidUrl)?;
    let endpoint = Url::parse_with_params(
        OEMBED_ENDPOINT,
        &[("url", video.watch_url.as_str()), ("format", "json")],
    )
    .map_err(|_| YoutubeError::Service)?;

    let response = reqwest::get(endpoint)
        .await
        .map_err(|error| {
            eprintln!("YouTube metadata request failed: {error}");
            YoutubeError::Service
        })?
        .error_for_status()
        .map_err(|error| {
            eprintln!("YouTube metadata response failed: {error}");
            YoutubeError::Unavailable
        })?
        .json::<OEmbedResponse>()
        .await
        .map_err(|error| {
            eprintln!("YouTube metadata response could not be decoded: {error}");
            YoutubeError::Unavailable
        })?;

    Ok(Json(VideoMetadata {
        video_id: video.id,
        title: response.title,
        author_name: response.author_name,
        author_url: response.author_url,
        thumbnail_url: response.thumbnail_url,
        embed_url: video.embed_url,
    }))
}

#[derive(Debug)]
struct YouTubeVideo {
    id: String,
    watch_url: Url,
    embed_url: String,
}

impl YouTubeVideo {
    fn parse(input: &str) -> Option<Self> {
        let url = Url::parse(input.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }

        let host = url.host_str()?.to_ascii_lowercase();
        let id = match host.as_str() {
            "youtu.be" => url.path_segments()?.next()?.to_owned(),
            "youtube.com" | "www.youtube.com" | "m.youtube.com" => {
                let path = url.path().trim_matches('/');
                if path == "watch" {
                    url.query_pairs()
                        .find(|(key, _)| key == "v")
                        .map(|(_, value)| value.into_owned())?
                } else if let Some(id) = path.strip_prefix("shorts/") {
                    id.to_owned()
                } else if let Some(id) = path.strip_prefix("embed/") {
                    id.to_owned()
                } else {
                    return None;
                }
            }
            _ => return None,
        };

        if !(6..=24).contains(&id.len())
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return None;
        }

        let watch_url = Url::parse(&format!("https://www.youtube.com/watch?v={id}")).ok()?;
        Some(Self {
            embed_url: format!("https://www.youtube.com/embed/{id}?autoplay=1&enablejsapi=1"),
            id,
            watch_url,
        })
    }
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
            let response = super::metadata(extracted).await.into_response();
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
        assert!(YouTubeVideo::parse("https://www.youtube.com/playlist?list=abc").is_none());
        assert!(YouTubeVideo::parse("https://example.com/watch?v=dQw4w9WgXcQ").is_none());
        assert!(YouTubeVideo::parse("https://youtu.be/short").is_none());
        assert!(YouTubeVideo::parse("javascript:alert(1)").is_none());
    }
}
