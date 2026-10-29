//! Supported-link normalization and bounded, reusable YouTube oEmbed lookup.
use crate::models::youtube::VideoMetadata;
use url::Url;

#[derive(Debug)]
pub enum MetadataError {
    InvalidUrl,
    Service,
    Unavailable,
}

#[derive(Debug)]
pub struct YouTubeVideo {
    pub id: String,
    pub watch_url: Url,
    pub embed_url: String,
}

impl YouTubeVideo {
    pub fn parse(_input: &str) -> Result<Self, MetadataError> {
        Self::parse_option(_input).ok_or(MetadataError::InvalidUrl)
    }
}

#[derive(Clone)]
pub struct MetadataClient {
    client: reqwest::Client,
    endpoint: Url,
}

impl MetadataClient {
    fn client() -> Result<reqwest::Client, reqwest::Error> {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
    }
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Self::client()?,
            endpoint: Url::parse("https://www.youtube.com/oembed").expect("fixed endpoint"),
        })
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: Url) -> Self {
        Self {
            client: Self::client().expect("test metadata client"),
            endpoint,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(endpoint: Url) -> Self {
        Self::with_endpoint(endpoint)
    }

    pub async fn lookup(&self, video: &YouTubeVideo) -> Result<VideoMetadata, MetadataError> {
        let mut endpoint = self.endpoint.clone();
        endpoint
            .query_pairs_mut()
            .append_pair("url", video.watch_url.as_str())
            .append_pair("format", "json");
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|_| MetadataError::Service)?;
        if !response.status().is_success()
            || response.content_length().is_some_and(|size| size > 65536)
        {
            return Err(MetadataError::Unavailable);
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                MetadataError::Service
            } else {
                MetadataError::Unavailable
            }
        })? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(MetadataError::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: OEmbedResponse =
            serde_json::from_slice(&bytes).map_err(|_| MetadataError::Unavailable)?;
        Ok(VideoMetadata {
            video_id: video.id.clone(),
            title: response.title,
            author_name: response.author_name,
            author_url: response.author_url,
            thumbnail_url: response.thumbnail_url,
            embed_url: video.embed_url.clone(),
        })
    }
}

#[derive(serde::Deserialize)]
struct OEmbedResponse {
    title: String,
    author_name: String,
    author_url: String,
    thumbnail_url: String,
}

#[cfg(test)]
mod tests;
impl YouTubeVideo {
    fn parse_option(input: &str) -> Option<Self> {
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
pub mod search;
