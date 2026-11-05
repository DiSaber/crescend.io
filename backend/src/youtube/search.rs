//! Bounded Data API search, separate from queue oEmbed validation.
use crate::models::search::{Snapshot, valid_thumbnail};
use std::collections::HashSet;

// search.list costs the same at any page size, so ask for more than we keep: the music
// filter below can discard most of a page. videos.list takes up to 50 IDs for one unit.
const CANDIDATES: usize = 25;
const MAX_RESULTS: usize = 10;
// Excludes Shorts and long mixes or loops from discovery results.
const MIN_SECONDS: u64 = 60;
const MAX_SECONDS: u64 = 15 * 60;

#[derive(Clone)]
pub struct SearchClient {
    client: reqwest::Client,
    endpoint: url::Url,
    videos_endpoint: url::Url,
    #[cfg(test)]
    skip_music_filter: bool,
}
impl SearchClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(2))
                .timeout(std::time::Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()?,
            endpoint: url::Url::parse("https://www.googleapis.com/youtube/v3/search")
                .expect("fixed search URL"),
            videos_endpoint: url::Url::parse("https://www.googleapis.com/youtube/v3/videos")
                .expect("fixed videos URL"),
            #[cfg(test)]
            skip_music_filter: false,
        })
    }
    #[cfg(test)]
    pub(crate) fn for_test(endpoint: url::Url) -> Self {
        Self {
            endpoint,
            skip_music_filter: true,
            ..Self::new().unwrap()
        }
    }
    #[cfg(test)]
    pub(crate) fn for_filter_test(endpoint: url::Url) -> Self {
        Self {
            videos_endpoint: endpoint.join("videos").unwrap(),
            endpoint,
            ..Self::new().unwrap()
        }
    }
    async fn get_json(&self, endpoint: url::Url, key: &str) -> Result<serde_json::Value, ()> {
        let mut header = reqwest::header::HeaderValue::from_str(key).map_err(|_| ())?;
        header.set_sensitive(true);
        let mut response = self
            .client
            .get(endpoint)
            .header("X-Goog-Api-Key", header)
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 262144)
        {
            return Err(());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if bytes.len() + chunk.len() > 262144 {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| ())
    }
    /// The IDs YouTube classifies as music and whose length suits a queue. A video the
    /// lookup omits is excluded; a failed lookup fails discovery, so unfiltered results
    /// are never returned or saved to the catalog.
    async fn music_ids(&self, ids: &[&str], key: &str) -> Result<HashSet<String>, ()> {
        let mut endpoint = self.videos_endpoint.clone();
        endpoint.query_pairs_mut().extend_pairs([
            ("part", "contentDetails,topicDetails"),
            ("id", ids.join(",").as_str()),
            ("maxResults", "50"),
        ]);
        let value = self.get_json(endpoint, key).await?;
        let items = value.get("items").and_then(|v| v.as_array()).ok_or(())?;
        Ok(items
            .iter()
            .filter(|item| is_music_length(item) && has_music_topic(item))
            .filter_map(|item| item["id"].as_str().map(str::to_owned))
            .collect())
    }
    pub async fn search(&self, query: &str, key: &str) -> Result<Vec<Snapshot>, ()> {
        let mut endpoint = self.endpoint.clone();
        endpoint.query_pairs_mut().extend_pairs([
            ("part", "snippet"),
            ("type", "video"),
            ("videoEmbeddable", "true"),
            ("videoCategoryId", "10"),
            ("maxResults", "25"),
            ("order", "relevance"),
            ("q", query),
        ]);
        let value = self.get_json(endpoint, key).await?;
        let items = value.get("items").and_then(|v| v.as_array()).ok_or(())?;
        let mut songs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for item in items {
            let parsed = (|| {
                if item["id"]["kind"].as_str()? != "youtube#video" {
                    return None;
                }
                let id = item["id"]["videoId"].as_str()?;
                let title = html_escape::decode_html_entities(item["snippet"]["title"].as_str()?)
                    .into_owned();
                let artist =
                    html_escape::decode_html_entities(item["snippet"]["channelTitle"].as_str()?)
                        .into_owned();
                let thumbnail_url = ["high", "medium", "default"]
                    .into_iter()
                    .filter_map(|quality| item["snippet"]["thumbnails"][quality]["url"].as_str())
                    .find(|u| valid_thumbnail(u))
                    .map(str::to_owned);
                let song = Snapshot {
                    video_id: id.into(),
                    title,
                    artist,
                    thumbnail_url,
                    watch_url: format!("https://www.youtube.com/watch?v={id}"),
                };
                song.valid().then_some(song)
            })();
            if let Some(song) = parsed {
                if songs.len() < CANDIDATES && seen.insert(song.video_id.clone()) {
                    songs.push(song);
                }
            } else {
                eprintln!("Invalid discovery item skipped");
            }
        }
        if !items.is_empty() && songs.is_empty() {
            return Err(());
        }
        #[cfg(test)]
        let skip_music_filter = self.skip_music_filter;
        #[cfg(not(test))]
        let skip_music_filter = false;
        if !songs.is_empty() && !skip_music_filter {
            let ids: Vec<&str> = songs.iter().map(|song| song.video_id.as_str()).collect();
            let music = self.music_ids(&ids, key).await?;
            songs.retain(|song| music.contains(&song.video_id));
        }
        // Rank before cutting, or an official video deep in the page would never be seen.
        songs.sort_by_key(|song| crate::search::rank::tier(song, false, query));
        songs.truncate(MAX_RESULTS);
        Ok(songs)
    }
}

/// videos.list topicCategories are Wikipedia URLs YouTube assigns itself, unlike the
/// uploader-chosen category that search filters on, e.g. `.../wiki/Music`,
/// `.../wiki/Hip_hop_music` or `.../wiki/Music_of_Asia`.
fn has_music_topic(item: &serde_json::Value) -> bool {
    item["topicDetails"]["topicCategories"]
        .as_array()
        .is_some_and(|topics| {
            topics.iter().filter_map(|t| t.as_str()).any(|topic| {
                let name = topic.rsplit('/').next().unwrap_or_default();
                name == "Music" || name.ends_with("_music") || name.starts_with("Music_of_")
            })
        })
}

fn is_music_length(item: &serde_json::Value) -> bool {
    item["contentDetails"]["duration"]
        .as_str()
        .and_then(iso_seconds)
        .is_some_and(|seconds| (MIN_SECONDS..=MAX_SECONDS).contains(&seconds))
}

/// Parses the ISO 8601 durations videos.list returns, such as `PT4M13S` or `P1DT2H`.
pub(crate) fn iso_seconds(duration: &str) -> Option<u64> {
    let mut rest = duration.strip_prefix('P')?;
    if rest.is_empty() {
        return None;
    }
    let mut total = 0u64;
    let mut number = String::new();
    let mut in_time = false;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        match c {
            '0'..='9' => number.push(c),
            'T' if number.is_empty() && !in_time => in_time = true,
            'D' | 'H' | 'M' | 'S' => {
                let scale = match (c, in_time) {
                    ('D', false) => 86400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
                let value: u64 = number.parse().ok()?;
                total = total.checked_add(value.checked_mul(scale)?)?;
                number.clear();
            }
            _ => return None,
        }
    }
    number.is_empty().then_some(total)
}
