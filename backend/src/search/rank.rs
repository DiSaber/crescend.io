//! Result ordering, so the version most people want comes first. Scoring uses only the
//! title and channel already stored, so it applies to catalog and saved results alike and
//! spends no provider quota. The sort is stable: within a tier, existing relevance holds.
use super::normalize::normalize;
use crate::models::search::Snapshot;

// Versions people usually want only when they ask for them.
const DERIVATIVE_WORDS: [&str; 15] = [
    "8d",
    "acapella",
    "cover",
    "covers",
    "instrumental",
    "karaoke",
    "mashup",
    "nightcore",
    "parody",
    "reaction",
    "remix",
    "reverb",
    "slowed",
    "sped",
    "tribute",
];

fn words(normalized: &str) -> impl Iterator<Item = &str> {
    normalized
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// Lower is better. `curated` marks hand-checked starter metadata. `query` is normalized.
pub fn tier(song: &Snapshot, curated: bool, query: &str) -> u8 {
    let title = normalize(&song.title);
    let channel = normalize(&song.artist);
    let derivative = |w: &str| DERIVATIVE_WORDS.contains(&w);
    let unrequested = words(&title)
        .chain(words(&channel))
        .any(|w| derivative(w) && !words(query).any(|q| q == w));
    if unrequested {
        return 4;
    }
    let official_channel = channel.ends_with("vevo")
        || channel.ends_with(" - topic")
        || words(&channel).any(|w| w == "official");
    if curated {
        return 0;
    }
    let lyric_upload = words(&title).any(|w| w == "lyrics" || w == "lyric");
    if official_channel {
        // The main version beats the same artist's lyric and live uploads, unless asked for.
        let alternate = lyric_upload
            || words(&title).any(|w| w == "live" && !words(query).any(|q| q == "live"));
        return u8::from(alternate);
    }
    // "Queen - Bohemian Rhapsody" from a channel named Queen is the artist's own upload.
    let own_upload = title
        .strip_prefix(&channel)
        .is_some_and(|rest| !channel.is_empty() && !rest.starts_with(char::is_alphanumeric));
    if own_upload || words(&title).any(|w| w == "official") {
        return 1;
    }
    if lyric_upload {
        return 3;
    }
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier_of(title: &str, artist: &str, query: &str) -> u8 {
        let song = Snapshot {
            video_id: "aaaaaa".into(),
            title: title.into(),
            artist: artist.into(),
            thumbnail_url: None,
            watch_url: "https://www.youtube.com/watch?v=aaaaaa".into(),
        };
        tier(&song, false, query)
    }

    #[test]
    fn official_channels_rank_above_uploads_lyrics_and_derivatives() {
        let q = "bohemian rhapsody";
        // Titles and channels as YouTube returned them.
        assert_eq!(tier_of("Crazy in Love", "BeyoncéVEVO", q), 0);
        assert_eq!(tier_of("Bohemian Rhapsody", "Queen - Topic", q), 0);
        assert_eq!(tier_of("Bohemian Rhapsody", "Queen Official", q), 0);
        assert_eq!(
            tier_of("The Kiffness - Xylophone Cat", "The Kiffness", q),
            1
        );
        assert_eq!(tier_of("Song (Official Video)", "Some Channel", q), 1);
        assert_eq!(
            tier_of(
                "Queen - Bohemian Rhapsody (Lyrics In Italian)",
                "Queen Official",
                q
            ),
            1
        );
        assert_eq!(tier_of("Song (Live at Reading 1992)", "NirvanaVEVO", q), 1);
        assert_eq!(
            tier_of("Song (Live at Reading 1992)", "NirvanaVEVO", "song live"),
            0
        );
        assert_eq!(
            tier_of("Bohemian Rhapsody | Muppet Music Video", "The Muppets", q),
            2
        );
        assert_eq!(
            tier_of("Queen - Bohemian Rhapsody (Lyrics)", "7clouds", q),
            3
        );
        assert_eq!(tier_of("Bohemian Rhapsody (Piano Cover)", "Someone", q), 4);
        assert_eq!(
            tier_of("Bohemian Rhapsody", "Sing King Karaoke - Topic", q),
            4
        );
    }

    #[test]
    fn asking_for_a_version_removes_its_penalty() {
        assert_eq!(tier_of("Song (Acoustic Cover)", "Someone", "song"), 4);
        assert_eq!(tier_of("Song (Acoustic Cover)", "Someone", "song cover"), 2);
        assert_eq!(tier_of("Song Remix", "ArtistVEVO", "song remix"), 0);
    }

    #[test]
    fn channel_name_must_match_whole_words() {
        assert_eq!(tier_of("Queensryche - Song", "Queen", "song"), 2);
        assert_eq!(tier_of("Trevor plays", "", "song"), 2);
    }
}
