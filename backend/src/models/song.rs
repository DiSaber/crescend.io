use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SongSearchQuery {
    /// Free text matched against title and artist: partial words, any case, accents optional.
    /// 1-100 characters after trimming.
    #[param(example = "crab ra")]
    pub q: String,
    /// Maximum results, 1-25. Defaults to 10.
    #[param(example = 10)]
    pub limit: Option<u32>,
}

/// One catalog song, ready to submit through the queue API as `url`.
#[derive(Debug, Serialize, ToSchema)]
pub struct SongResult {
    #[schema(example = "LDU_Txk06tM")]
    pub video_id: String,
    /// Display title with HTML entities decoded.
    #[schema(example = "Crab Rave")]
    pub title: String,
    /// Artist for starter songs, otherwise the uploading channel.
    #[schema(example = "Noisestorm")]
    pub artist: String,
    #[schema(example = "https://i.ytimg.com/vi/LDU_Txk06tM/hqdefault.jpg")]
    pub thumbnail_url: String,
    /// Watch URL accepted by `POST .../queue/entries`.
    #[schema(example = "https://www.youtube.com/watch?v=LDU_Txk06tM")]
    pub url: String,
    /// Accepted queue submissions of this video across all lobbies.
    pub times_queued: i64,
}

/// Whether YouTube was searched for this request. Only the catalog exists today; the
/// YouTube Data API fallback (#41 part 2) adds the other outcomes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum YouTubeSearchStatus {
    /// No YouTube search is configured; results are catalog-only.
    NotConfigured,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SongSearchResponse {
    /// Best match first: whole-word matches before prefix-only matches, then most queued.
    pub results: Vec<SongResult>,
    pub youtube: YouTubeSearchStatus,
}
