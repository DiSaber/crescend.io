use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub thumbnail_url: Option<String>,
    pub watch_url: String,
}

impl Snapshot {
    pub fn valid(&self) -> bool {
        crate::youtube::YouTubeVideo::parse(&self.watch_url)
            .is_ok_and(|v| v.id == self.video_id && v.watch_url.as_str() == self.watch_url)
            && !self.title.trim().is_empty()
            && !self.artist.trim().is_empty()
            && self.title.len() <= 16384
            && self.artist.len() <= 16384
            && self.thumbnail_url.as_deref().is_none_or(valid_thumbnail)
    }
}

pub fn valid_thumbnail(value: &str) -> bool {
    value.len() <= 8192
        && url::Url::parse(value).is_ok_and(|u| {
            u.scheme() == "https"
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
        })
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Catalog,
    Youtube,
    YoutubeCache,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct SearchResult {
    #[serde(flatten)]
    pub song: Snapshot,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStatus {
    NotNeeded,
    Cached,
    Fresh,
    MissingCredentials,
    DailyLimitReached,
    TemporarilyUnavailable,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Discovery {
    pub status: DiscoveryStatus,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub discovery: Discovery,
}

#[derive(Clone, Debug)]
pub struct FlightOutcome {
    pub results: Vec<Snapshot>,
    pub status: DiscoveryStatus,
}

impl FlightOutcome {
    pub fn status(status: DiscoveryStatus) -> Self {
        Self {
            results: vec![],
            status,
        }
    }
}
