use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetadataRequest {
    /// YouTube watch, shorts, embed, or youtu.be video URL.
    #[schema(example = "https://youtu.be/dQw4w9WgXcQ")]
    pub url: String,
}

/// Normalized YouTube video details with metadata retrieved from YouTube oEmbed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VideoMetadata {
    /// Normalized video identifier, suitable for identifying queue entries.
    #[schema(example = "dQw4w9WgXcQ")]
    pub video_id: String,
    /// Video title returned by YouTube.
    #[schema(example = "Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster)")]
    pub title: String,
    /// Display name of the channel that uploaded the video.
    #[schema(example = "Rick Astley")]
    pub author_name: String,
    /// URL of the uploader's YouTube channel.
    #[schema(example = "https://www.youtube.com/@RickAstleyYT")]
    pub author_url: String,
    /// URL of the video thumbnail returned by YouTube.
    #[schema(example = "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg")]
    pub thumbnail_url: String,
    /// Player URL with autoplay and the JavaScript player API enabled.
    #[schema(example = "https://www.youtube.com/embed/dQw4w9WgXcQ?autoplay=1&enablejsapi=1")]
    pub embed_url: String,
}
