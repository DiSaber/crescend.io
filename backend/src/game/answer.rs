//! Name That Tune answer keys: cleaning YouTube titles and forgiving title matching.
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Guesses longer than this are rejected before matching.
pub const MAX_GUESS_CHARS: usize = 100;
/// Wrong guesses each player may make per round.
pub const MAX_WRONG_GUESSES: i64 = 10;

/// Words that alone mark a title segment as packaging ("Official Video", "[4K]").
const STRONG_JUNK: &[&str] = &[
    "official",
    "video",
    "lyric",
    "lyrics",
    "audio",
    "hd",
    "hq",
    "4k",
    "8k",
    "remaster",
    "remastered",
    "visualizer",
    "visualiser",
    "mv",
    "m/v",
];

/// Words that are packaging only next to a strong word ("Official Music Video").
const WEAK_JUNK: &[&str] = &[
    "music", "clip", "explicit", "version", "full", "color", "coded", "with", "new", "1080p",
    "720p", "and",
];

/// Words a guess may leave out without being wrong.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "and", "to", "in", "on", "at", "for", "my", "is", "it", "by", "from",
];

/// Tails that introduce featured artists. Matched case-insensitively on word boundaries.
const FEATURING: &[&str] = &[
    " ft. ",
    " ft ",
    " feat. ",
    " feat ",
    " featuring ",
    " prod. ",
];

const SEPARATORS: &[&str] = &[" - ", " – ", " — ", " | ", " // ", " ~ "];

/// The song name from a YouTube title: drops bracketed suffixes, featured-artist
/// tails, a leading "Artist - " and segments that only describe the upload.
pub fn clean_title(raw: &str) -> String {
    let decoded = html_escape::decode_html_entities(raw);
    let mut text = strip_brackets(&decoded);
    text.push(' ');
    if let Some(cut) = FEATURING
        .iter()
        .filter_map(|tail| find_ignore_case(&text, tail))
        .min()
    {
        text.truncate(cut);
    }
    let mut parts: Vec<String> = vec![text];
    for separator in SEPARATORS {
        parts = parts
            .iter()
            .flat_map(|part| part.split(separator).map(str::to_owned).collect::<Vec<_>>())
            .collect();
    }
    let parts: Vec<String> = parts
        .into_iter()
        .map(|part| trim_title(&part))
        .filter(|part| !part.is_empty() && !all_junk(part))
        .collect();
    let chosen = match parts.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [_artist, title, ..] => title.clone(),
    };
    let chosen = drop_trailing_junk(&chosen);
    if chosen.is_empty() {
        raw.trim().to_owned()
    } else {
        chosen
    }
}

/// Byte offset of an ASCII `needle` in `text`, ignoring ASCII case.
fn find_ignore_case(text: &str, needle: &str) -> Option<usize> {
    text.char_indices().map(|(i, _)| i).find(|&i| {
        text.get(i..i + needle.len())
            .is_some_and(|window| window.eq_ignore_ascii_case(needle))
    })
}

fn strip_brackets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' | '[' | '{' | '【' | '「' | '『' => depth += 1,
            ')' | ']' | '}' | '】' | '」' | '』' if depth > 0 => {
                depth -= 1;
                out.push(' ');
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn trim_title(part: &str) -> String {
    let words: Vec<&str> = part.split_whitespace().collect();
    words
        .join(" ")
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’' | ',' | ':' | '-'))
        .trim()
        .to_owned()
}

fn junk_word(word: &str) -> String {
    word.to_lowercase()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '/')
        .to_owned()
}

fn is_strong_junk(word: &str) -> bool {
    STRONG_JUNK.contains(&junk_word(word).as_str())
}

/// Only packaging words, at least one of them strong: "Official Music Video", "HD".
fn all_junk(part: &str) -> bool {
    let words: Vec<&str> = part.split_whitespace().collect();
    words.iter().any(|w| is_strong_junk(w))
        && words
            .iter()
            .all(|w| is_strong_junk(w) || WEAK_JUNK.contains(&junk_word(w).as_str()))
}

fn drop_trailing_junk(part: &str) -> String {
    let mut words: Vec<&str> = part.split_whitespace().collect();
    while words.len() > 1 && words.last().is_some_and(|w| is_strong_junk(w)) {
        words.pop();
    }
    trim_title(&words.join(" "))
}

/// Case, accents and punctuation removed; apostrophes join words ("don't" -> "dont").
pub fn normalize(value: &str) -> String {
    let folded: String = value
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .filter(|c| !matches!(c, '\'' | '’' | '`'))
        .map(|c| if c == '&' { ' ' } else { c })
        .collect();
    let spaced: String = folded
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let words: Vec<&str> = spaced.split_whitespace().collect();
    words.join(" ")
}

fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[b.len()]
}

/// Typos allowed for an answer of `length` letters (spaces ignored).
fn tolerance(length: usize) -> usize {
    match length {
        0..=3 => 0,
        4..=8 => 1,
        9..=14 => 2,
        _ => length / 7,
    }
}

/// The answer key without its optional parts: bracketed or parenthesized text and
/// a featured-artist tail ("Old Town Road (Remix)" -> "Old Town Road"). The same
/// cleanup YouTube titles get, minus the artist split, since an answer key holds
/// only the song name.
pub fn core_title(answer: &str) -> String {
    let mut text = strip_brackets(answer);
    text.push(' ');
    if let Some(cut) = FEATURING
        .iter()
        .filter_map(|tail| find_ignore_case(&text, tail))
        .min()
    {
        text.truncate(cut);
    }
    drop_trailing_junk(&trim_title(&text))
}

/// Whether a guess names the song: it matches the full answer key, or the key
/// with its optional parts left out, so "Dracula" and "Dracula (JENNIE Remix)"
/// both count for "Dracula (JENNIE Remix)".
pub fn accepts(guess: &str, answer: &str) -> bool {
    let core = core_title(answer);
    let keys = [answer, core.as_str()];
    // The guess may carry the same optional parts ("Dracula [Official Video]").
    let short = core_title(guess);
    [guess, short.as_str()]
        .iter()
        .filter(|g| !g.trim().is_empty())
        .any(|g| keys.iter().filter(|k| !k.is_empty()).any(|k| matches(g, k)))
}

/// A guess matches when, after normalization, it is within a small edit distance of
/// the answer (relative to its length), or it contains every significant answer word
/// (words of five or more letters may have one typo) with at most three extra words.
pub fn matches(guess: &str, answer: &str) -> bool {
    let (guess, answer) = (normalize(guess), normalize(answer));
    if guess.is_empty() || answer.is_empty() {
        return false;
    }
    let compact = |s: &str| s.replace(' ', "");
    let (g, a) = (compact(&guess), compact(&answer));
    if distance(&g, &a) <= tolerance(a.chars().count()) {
        return true;
    }
    let guess_words: Vec<&str> = guess.split(' ').filter(|w| *w != "and").collect();
    let answer_words: Vec<&str> = answer.split(' ').collect();
    let mut significant: Vec<&str> = answer_words
        .iter()
        .copied()
        .filter(|w| !STOPWORDS.contains(w))
        .collect();
    if significant.is_empty() {
        significant = answer_words.clone();
    }
    guess_words.len() <= answer_words.len() + 3
        && significant.iter().all(|word| {
            guess_words
                .iter()
                .any(|g| g == word || (word.chars().count() >= 5 && distance(g, word) <= 1))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_real_youtube_titles_to_the_song_name() {
        for (raw, clean) in [
            (
                "Queen - Bohemian Rhapsody (Official Video Remastered)",
                "Bohemian Rhapsody",
            ),
            (
                "Nirvana - Smells Like Teen Spirit (Official Music Video)",
                "Smells Like Teen Spirit",
            ),
            ("Luis Fonsi - Despacito ft. Daddy Yankee", "Despacito"),
            (
                "Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster)",
                "Never Gonna Give You Up",
            ),
            (
                "Gotye - Somebody That I Used To Know (feat. Kimbra) [Official Music Video]",
                "Somebody That I Used To Know",
            ),
            (
                "Mark Ronson - Uptown Funk (Official Video) ft. Bruno Mars",
                "Uptown Funk",
            ),
            (
                "Daft Punk - Get Lucky (Official Audio) ft. Pharrell Williams, Nile Rodgers",
                "Get Lucky",
            ),
            ("PSY - GANGNAM STYLE(강남스타일) M/V", "GANGNAM STYLE"),
            ("a-ha - Take On Me (Official Video) [4K]", "Take On Me"),
            ("Toto - Africa (Official HD Video)", "Africa"),
            ("Beyoncé - Halo", "Halo"),
            ("Ed Sheeran - Shape of You [Official Video]", "Shape of You"),
            (
                "The Weeknd - Blinding Lights | Official Video",
                "Blinding Lights",
            ),
            ("Kendrick Lamar - HUMBLE.", "HUMBLE."),
            (
                "Guns N&#39; Roses - Sweet Child O&#39; Mine (Official Music Video)",
                "Sweet Child O' Mine",
            ),
            ("Adele - Hello", "Hello"),
            ("Bohemian Rhapsody", "Bohemian Rhapsody"),
            (
                "Coldplay - Viva La Vida (Official Video) - Lyrics",
                "Viva La Vida",
            ),
            ("OutKast - Hey Ya! (Official HD Video)", "Hey Ya!"),
            ("(Official Video)", "(Official Video)"),
            ("Taylor Swift - Clean", "Clean"),
            ("Madonna - Music (Official Video)", "Music"),
            ("Dua Lipa - New Rules (Official Music Video)", "New Rules"),
        ] {
            assert_eq!(clean_title(raw), clean, "{raw}");
        }
    }

    #[test]
    fn matching_forgives_case_accents_punctuation_and_small_typos() {
        for (guess, answer) in [
            ("bohemian rhapsody", "Bohemian Rhapsody"),
            ("BOHEMIAN RHAPSODY!!!", "Bohemian Rhapsody"),
            ("bohemian rapsody", "Bohemian Rhapsody"),
            ("queen bohemian rhapsody", "Bohemian Rhapsody"),
            ("smells like teen spirit", "Smells Like Teen Spirit"),
            ("smell like teen spirit", "Smells Like Teen Spirit"),
            (
                "Smells Like Teen Spirit by Nirvana",
                "Smells Like Teen Spirit",
            ),
            ("despasito", "Despacito"),
            ("DESPACITO", "Despacito"),
            ("beyonce halo", "Halo"),
            ("hallo", "Halo"),
            ("never gonna give you up", "Never Gonna Give You Up"),
            ("nevr gonna give you up", "Never Gonna Give You Up"),
            ("sweet child of mine", "Sweet Child O' Mine"),
            ("sweet child o mine", "Sweet Child O' Mine"),
            ("dont stop believin", "Don't Stop Believin'"),
            ("choosin texas", "Choosin' Texas"),
            ("shape of you", "Shape of You"),
            ("shape you", "Shape of You"),
            ("hey ya", "Hey Ya!"),
            ("humble", "HUMBLE."),
        ] {
            assert!(matches(guess, answer), "{guess:?} should match {answer:?}");
        }
    }

    #[test]
    fn matching_rejects_partial_wrong_and_spammed_guesses() {
        for (guess, answer) in [
            ("bohemian", "Bohemian Rhapsody"),
            ("queen", "Bohemian Rhapsody"),
            ("smells like teen", "Smells Like Teen Spirit"),
            ("teen spirit", "Smells Like Teen Spirit"),
            ("despacito 2", "Hello"),
            ("hello", "Halo"),
            ("hell", "Halo"),
            ("never gonna", "Never Gonna Give You Up"),
            (
                "bohemian rhapsody smells like teen spirit despacito hello",
                "Bohemian Rhapsody",
            ),
            ("", "Halo"),
            ("!!!", "Halo"),
            ("africa", "America"),
        ] {
            assert!(
                !matches(guess, answer),
                "{guess:?} should not match {answer:?}"
            );
        }
    }

    #[test]
    fn curated_titles_take_their_bracketed_parts_and_featured_artists_as_optional() {
        for (guess, answer) in [
            ("Old Town Road", "Old Town Road (Remix)"),
            ("old town road remix", "Old Town Road (Remix)"),
            ("Old Town Road (Remix)", "Old Town Road (Remix)"),
            ("Dracula", "Dracula (JENNIE Remix)"),
            ("dracula jennie remix", "Dracula (JENNIE Remix)"),
            ("A Bar Song", "A Bar Song (Tipsy)"),
            ("a bar song tipsy", "A Bar Song (Tipsy)"),
            ("Waka Waka", "Waka Waka (This Time for Africa)"),
            (
                "Somebody That I Used To Know",
                "Somebody That I Used To Know (feat. Kimbra)",
            ),
            ("Despacito", "Despacito ft. Daddy Yankee"),
            ("Old Town Road ft. Billy Ray Cyrus", "Old Town Road (Remix)"),
            ("Dracula [Official Video]", "Dracula (JENNIE Remix)"),
            ("Bohemian Rhapsody ft. Queen", "Bohemian Rhapsody"),
            ("Choosin Texas", "Choosin' Texas"),
        ] {
            assert!(accepts(guess, answer), "{guess:?} should name {answer:?}");
        }
        for (guess, answer) in [
            ("Tipsy", "A Bar Song (Tipsy)"),
            ("Remix", "Old Town Road (Remix)"),
            ("Old Town", "Old Town Road (Remix)"),
            ("Lil Nas X", "Old Town Road (Remix)"),
        ] {
            assert!(
                !accepts(guess, answer),
                "{guess:?} should not name {answer:?}"
            );
        }
        assert_eq!(core_title("Old Town Road (Remix)"), "Old Town Road");
        assert_eq!(core_title("A Bar Song (Tipsy)"), "A Bar Song");
        assert_eq!(
            core_title("(Remix)"),
            "",
            "nothing left: the full key is the only answer"
        );
        assert_eq!(core_title("Bohemian Rhapsody"), "Bohemian Rhapsody");
    }

    #[test]
    fn normalize_folds_accents_apostrophes_and_symbols() {
        assert_eq!(normalize("  Beyoncé — Halo!  "), "beyonce halo");
        assert_eq!(normalize("Don't Stop Believin'"), "dont stop believin");
        assert_eq!(normalize("Guns N' Roses & Friends"), "guns n roses friends");
    }
}
