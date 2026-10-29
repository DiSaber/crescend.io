//! Song catalog domain: starter list, search parsing and HTML entity decoding.
use crate::database::lobbies::LobbyError;

/// Checked into the repo and compiled in, so every deployment starts with the same songs.
const STARTER_SONGS: &str = include_str!("../../data/starter-songs.json");

pub const DEFAULT_LIMIT: u32 = 10;
pub const MAX_LIMIT: u32 = 25;
const MAX_QUERY_CHARS: usize = 100;
const MAX_TERMS: usize = 8;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StarterSong {
    pub video_id: String,
    pub title: String,
    pub artist: String,
}

pub fn starter_songs() -> Vec<StarterSong> {
    serde_json::from_str(STARTER_SONGS).expect("data/starter-songs.json must be a valid song list")
}

pub fn watch_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

pub fn thumbnail_url(video_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")
}

/// A validated search: the words to match and how many results to return.
#[derive(Debug)]
pub struct SongSearch {
    terms: Vec<String>,
    pub limit: u32,
}

impl SongSearch {
    /// Splits on anything that isn't a letter or digit, as the FTS5 tokenizer does, so
    /// punctuation and FTS5 syntax in user text can never reach the MATCH expression.
    pub fn parse(q: &str, limit: Option<u32>) -> Result<Self, SongsError> {
        let q = q.trim();
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        if q.is_empty() || q.chars().count() > MAX_QUERY_CHARS || !(1..=MAX_LIMIT).contains(&limit)
        {
            return Err(SongsError::InvalidQuery);
        }
        let terms = q
            .split(|c: char| !c.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .take(MAX_TERMS)
            .map(str::to_owned)
            .collect();
        Ok(Self { terms, limit })
    }

    /// True when the text had no letters or digits; such a search matches nothing.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Every term as a word prefix: "cra ra" matches "Crab Rave".
    pub fn prefix_query(&self) -> String {
        self.terms
            .iter()
            .map(|t| format!("\"{t}\"*"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Every term as a whole word, for ranking full-word matches first.
    pub fn word_query(&self) -> String {
        self.terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Decodes the entities YouTube puts in titles (`&amp;`, `&#39;`, `&quot;`, ...) in one
/// pass, so "&amp;amp;" becomes "&amp;" rather than "&". Unknown entities are kept as text.
pub fn decode_html_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| {
                let entity = &rest[1..=end];
                let c = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    _ => entity
                        .strip_prefix("#x")
                        .or_else(|| entity.strip_prefix("#X"))
                        .map(|hex| u32::from_str_radix(hex, 16))
                        .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                        .and_then(Result::ok)
                        .and_then(char::from_u32)
                        .filter(|c| *c != '\0'),
                };
                c.map(|c| (c, end + 2))
            });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug)]
pub enum SongsError {
    Unauthorized,
    InvalidQuery,
    Unavailable,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for SongsError {
    fn from(error: sqlx::Error) -> Self {
        match LobbyError::from(error) {
            LobbyError::Unavailable => Self::Unavailable,
            LobbyError::Database(error) => Self::Database(error),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests;
