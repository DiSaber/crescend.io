//! Bounded Data API search, separate from queue oEmbed validation.
use crate::models::search::{Snapshot, valid_thumbnail};
#[derive(Clone)]
pub struct SearchClient {
    client: reqwest::Client,
    endpoint: url::Url,
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
        })
    }
    #[cfg(test)]
    pub(crate) fn for_test(endpoint: url::Url) -> Self {
        Self {
            endpoint,
            ..Self::new().unwrap()
        }
    }
    pub async fn search(&self, query: &str, key: &str) -> Result<Vec<Snapshot>, ()> {
        let mut header = reqwest::header::HeaderValue::from_str(key).map_err(|_| ())?;
        header.set_sensitive(true);
        let mut endpoint = self.endpoint.clone();
        endpoint.query_pairs_mut().extend_pairs([
            ("part", "snippet"),
            ("type", "video"),
            ("videoEmbeddable", "true"),
            ("videoCategoryId", "10"),
            ("maxResults", "10"),
            ("order", "relevance"),
            ("q", query),
        ]);
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
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
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
                if songs.len() < 10 && seen.insert(song.video_id.clone()) {
                    songs.push(song);
                }
            } else {
                eprintln!("Invalid discovery item skipped");
            }
        }
        if !items.is_empty() && songs.is_empty() {
            return Err(());
        }
        Ok(songs)
    }
}
