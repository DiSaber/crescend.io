use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use url::Url;

const OEMBED_ENDPOINT: &str = "https://www.youtube.com/oembed";

pub fn router() -> Router<crate::app_state::AppState> {
    Router::new().route("/metadata", post(metadata))
}

#[derive(Debug, Deserialize)]
pub struct MetadataRequest {
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct VideoMetadata {
    pub video_id: String,
    pub title: String,
    pub author_name: String,
    pub author_url: String,
    pub thumbnail_url: String,
    pub embed_url: String,
}

#[derive(Debug, Deserialize)]
struct OEmbedResponse {
    title: String,
    author_name: String,
    author_url: String,
    thumbnail_url: String,
}

async fn metadata(
    Json(request): Json<MetadataRequest>,
) -> Result<Json<VideoMetadata>, YoutubeError> {
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
    InvalidUrl,
    Service,
    Unavailable,
}

impl IntoResponse for YoutubeError {
    fn into_response(self) -> Response {
        match self {
            Self::InvalidUrl => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "The URL must be a valid YouTube video link.",
            )
                .into_response(),
            Self::Service => (
                StatusCode::BAD_GATEWAY,
                "The YouTube metadata service could not be reached.",
            )
                .into_response(),
            Self::Unavailable => (
                StatusCode::BAD_GATEWAY,
                "YouTube metadata is unavailable for this video.",
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::YouTubeVideo;

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
