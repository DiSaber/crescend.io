//! Per-lobby display names. A name belongs to one (lobby, account) pair, so the same
//! person can use different names in different lobbies. Rows outlive the membership
//! (they go with the lobby), which keeps departed players named on the scoreboard and
//! in "added by", and keeps their name reserved if they come back.
use super::Database;
use chrono::Utc;
use sqlx::SqliteConnection;
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;

/// Most user-perceived characters (grapheme clusters) in a name.
pub const MAX_CHARS: usize = 24;
/// Code points one character may use: a family emoji with skin tones needs 11.
/// This bounds stacked combining marks.
pub const MAX_SCALARS_PER_CHAR: usize = 12;
/// Code points a stored name may use; the tables' CHECK (SQLite length()) matches.
pub const MAX_SCALARS: usize = MAX_CHARS * MAX_SCALARS_PER_CHAR;
/// Used when a client sends no nickname and the account has never chosen one.
pub const DEFAULT_GUEST: &str = "Player";
pub const DEFAULT_HOST: &str = "Host";

/// Trims, collapses inner whitespace runs to one space and checks the rules:
/// 1-24 user-perceived characters, no control or invisible formatting characters.
/// Zero-width joiners and variation selectors are allowed only inside emoji
/// sequences (family and flag emoji), where they draw one visible picture.
pub fn normalize(raw: &str) -> Option<String> {
    display_name(raw, MAX_CHARS).ok()
}

/// Why a display name (nickname or game mode name) is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    /// A control, invisible or direction-changing character, or a joiner or
    /// variation selector outside an emoji sequence.
    Hidden,
    /// Empty after trimming, or more than the allowed characters.
    Length,
    /// One character stacks more than [`MAX_SCALARS_PER_CHAR`] code points.
    Stacked,
}

/// The display-name rules shared by nicknames and saved game mode names: trimmed,
/// inner whitespace runs collapsed to one space, 1 to `max_chars` user-perceived
/// characters (grapheme clusters, so a family emoji counts once), each at most
/// [`MAX_SCALARS_PER_CHAR`] code points, and no hidden characters (see `forbidden`;
/// ZWJ and variation selectors only inside emoji sequences).
pub fn display_name(raw: &str, max_chars: usize) -> Result<String, NameProblem> {
    let chars: Vec<char> = raw.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let before = i.checked_sub(1).map(|j| chars[j]);
        let after = chars.get(i + 1).copied();
        let allowed = match c {
            // A ZWJ glues two emoji: 👨‍👩‍👧, 🏳️‍🌈.
            '\u{200D}' => before.is_some_and(emoji_part) && after.is_some_and(pictographic),
            // Emoji or text presentation of the emoji (or keycap base, 1️⃣) before it.
            '\u{FE0E}' | '\u{FE0F}' => before
                .is_some_and(|b| pictographic(b) || b.is_ascii_digit() || b == '#' || b == '*'),
            _ => !forbidden(c),
        };
        if !allowed {
            return Err(NameProblem::Hidden);
        }
    }
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut count = 0;
    for grapheme in name.graphemes(true) {
        count += 1;
        if grapheme.chars().count() > MAX_SCALARS_PER_CHAR {
            return Err(NameProblem::Stacked);
        }
    }
    if (1..=max_chars).contains(&count) {
        Ok(name)
    } else {
        Err(NameProblem::Length)
    }
}

/// Control characters (Cc), variation selectors outside emoji, and the zero-width and
/// bidirectional formatting characters that can make two names look alike or reorder
/// surrounding text.
fn forbidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
            | '\u{FE00}'..='\u{FE0F}')
}

/// Emoji pictographs: an approximation of Unicode's Extended_Pictographic that covers
/// every emoji ZWJ sequence in use.
fn pictographic(c: char) -> bool {
    matches!(c, '\u{00A9}' | '\u{00AE}' | '\u{203C}' | '\u{2049}' | '\u{2122}' | '\u{2139}'
        | '\u{2194}'..='\u{21FF}' | '\u{2300}'..='\u{23FF}' | '\u{24C2}' | '\u{25A0}'..='\u{27BF}'
        | '\u{2900}'..='\u{297F}' | '\u{2B00}'..='\u{2BFF}' | '\u{3030}' | '\u{303D}' | '\u{3297}'
        | '\u{3299}' | '\u{1F000}'..='\u{1FAFF}')
}

/// What may come right before a ZWJ: an emoji, its presentation selector, a skin tone
/// (inside the 1F000 block) or the end of a tag sequence.
fn emoji_part(c: char) -> bool {
    pictographic(c) || c == '\u{FE0F}' || c == '\u{E007F}'
}

/// User-perceived characters (grapheme clusters), as the 24-character rule counts them.
pub fn length(name: &str) -> usize {
    name.graphemes(true).count()
}

/// Case-insensitive key for uniqueness within a lobby.
fn fold(name: &str) -> String {
    name.to_lowercase()
}

/// `base`, then `base 2`, `base 3`, ... shortened so the suffix still fits in 24 characters.
fn candidate(base: &str, n: u32) -> String {
    if n == 1 {
        return base.to_owned();
    }
    let suffix = format!(" {n}");
    let keep = MAX_CHARS - suffix.len();
    let stem: String = base.graphemes(true).take(keep).collect();
    format!("{}{suffix}", stem.trim_end())
}

/// Shown for an account with no stored name in this lobby (rows from before nicknames).
pub fn fallback(user: i64) -> String {
    format!("{DEFAULT_GUEST} {user}")
}

/// All names ever used in the lobby, keyed by account.
pub(crate) async fn lobby_names(
    conn: &mut SqliteConnection,
    lobby: &str,
) -> Result<HashMap<i64, String>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (i64, String)>(
        "SELECT user_id, nickname FROM lobby_nicknames WHERE lobby_id = ?",
    )
    .bind(lobby)
    .fetch_all(conn)
    .await?
    .into_iter()
    .collect())
}

pub(crate) fn name_of(names: &HashMap<i64, String>, user: i64) -> String {
    names.get(&user).cloned().unwrap_or_else(|| fallback(user))
}

pub(crate) async fn current(
    conn: &mut SqliteConnection,
    lobby: &str,
    user: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT nickname FROM lobby_nicknames WHERE lobby_id = ? AND user_id = ?")
        .bind(lobby)
        .bind(user)
        .fetch_optional(conn)
        .await
}

pub(crate) async fn last(
    conn: &mut SqliteConnection,
    user: i64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT nickname FROM user_nicknames WHERE user_id = ?")
        .bind(user)
        .fetch_optional(conn)
        .await
}

/// Gives `user` the first free form of `wanted` in the lobby (call inside the writer
/// transaction) and remembers `wanted` as the account's last choice. Returns the stored
/// name and whether it changed.
pub(crate) async fn assign(
    conn: &mut SqliteConnection,
    lobby: &str,
    user: i64,
    wanted: &str,
    remember: bool,
) -> Result<(String, bool), sqlx::Error> {
    let before = current(&mut *conn, lobby, user).await?;
    let mut chosen = None;
    for n in 1..=10_000 {
        let name = candidate(wanted, n);
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM lobby_nicknames WHERE lobby_id = ? AND folded = ? AND user_id != ?)",
        )
        .bind(lobby)
        .bind(fold(&name))
        .bind(user)
        .fetch_one(&mut *conn)
        .await?;
        if !taken {
            chosen = Some(name);
            break;
        }
    }
    let name = chosen.ok_or_else(|| sqlx::Error::Protocol("no free nickname".into()))?;
    if before.as_deref() != Some(name.as_str()) {
        sqlx::query(
            "INSERT INTO lobby_nicknames (lobby_id, user_id, nickname, folded) VALUES (?, ?, ?, ?)
             ON CONFLICT(lobby_id, user_id) DO UPDATE SET nickname = excluded.nickname, folded = excluded.folded",
        )
        .bind(lobby)
        .bind(user)
        .bind(&name)
        .bind(fold(&name))
        .execute(&mut *conn)
        .await?;
    }
    if remember {
        sqlx::query(
            "INSERT INTO user_nicknames (user_id, nickname, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET nickname = excluded.nickname, updated_at = excluded.updated_at",
        )
        .bind(user)
        .bind(wanted)
        .bind(Utc::now().timestamp())
        .execute(&mut *conn)
        .await?;
    }
    let changed = before.as_deref() != Some(name.as_str());
    Ok((name, changed))
}

/// The name to use when a request carries none: the one already held in this lobby,
/// else the account's last choice, else `default`.
pub(crate) async fn resolve(
    conn: &mut SqliteConnection,
    lobby: &str,
    user: i64,
    requested: Option<&str>,
    default: &str,
) -> Result<(String, bool), sqlx::Error> {
    if let Some(name) = requested {
        return Ok((name.to_owned(), true));
    }
    if let Some(name) = current(&mut *conn, lobby, user).await? {
        return Ok((name, false));
    }
    Ok((
        last(&mut *conn, user)
            .await?
            .unwrap_or_else(|| default.to_owned()),
        false,
    ))
}

impl Database {
    pub(super) async fn create_nickname_tables(&self) -> Result<(), sqlx::Error> {
        // New tables only: existing users/membership rows are not rebuilt. The CHECK
        // counts code points (SQLite length()), MAX_SCALARS of them; the 24-character
        // rule counts what the eye sees, so a 7-code-point family emoji is one.
        const _: () = assert!(MAX_SCALARS == 288);
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS lobby_nicknames (
                lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                user_id INTEGER NOT NULL REFERENCES users(id),
                nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 288),
                folded TEXT NOT NULL,
                PRIMARY KEY(lobby_id, user_id),
                UNIQUE(lobby_id, folded)
            ) STRICT",
        )
        .execute(&self.db_pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_nicknames (
                user_id INTEGER PRIMARY KEY NOT NULL REFERENCES users(id),
                nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 288),
                updated_at INTEGER NOT NULL
            ) STRICT",
        )
        .execute(&self.db_pool)
        .await?;
        // Tables from before emoji sequences capped code points at 24: copy them into
        // the wider ones. Nothing references these tables.
        let old: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table'
             AND name IN ('lobby_nicknames', 'user_nicknames') AND sql LIKE '%BETWEEN 1 AND 24)%'",
        )
        .fetch_one(&self.db_pool)
        .await?;
        if old > 0 {
            let mut tx = self.db_pool.begin().await?;
            for statement in [
                "CREATE TABLE lobby_nicknames_wide (
                    lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                    user_id INTEGER NOT NULL REFERENCES users(id),
                    nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 288),
                    folded TEXT NOT NULL,
                    PRIMARY KEY(lobby_id, user_id),
                    UNIQUE(lobby_id, folded)
                ) STRICT",
                "INSERT INTO lobby_nicknames_wide (lobby_id, user_id, nickname, folded)
                    SELECT lobby_id, user_id, nickname, folded FROM lobby_nicknames",
                "DROP TABLE lobby_nicknames",
                "ALTER TABLE lobby_nicknames_wide RENAME TO lobby_nicknames",
                "CREATE TABLE user_nicknames_wide (
                    user_id INTEGER PRIMARY KEY NOT NULL REFERENCES users(id),
                    nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 288),
                    updated_at INTEGER NOT NULL
                ) STRICT",
                "INSERT INTO user_nicknames_wide (user_id, nickname, updated_at)
                    SELECT user_id, nickname, updated_at FROM user_nicknames",
                "DROP TABLE user_nicknames",
                "ALTER TABLE user_nicknames_wide RENAME TO user_nicknames",
            ] {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    pub async fn last_nickname(&self, user: i64) -> Result<Option<String>, sqlx::Error> {
        let mut conn = self.db_pool.acquire().await?;
        last(&mut conn, user).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nickname_rules() {
        assert_eq!(normalize("  Sam  ").as_deref(), Some("Sam"));
        assert_eq!(normalize("DJ \u{3000}  Kat").as_deref(), Some("DJ Kat"));
        assert_eq!(
            normalize("x".repeat(24).as_str()).map(|s| s.len()),
            Some(24)
        );
        assert_eq!(
            normalize(&"é".repeat(24)).map(|s| s.chars().count()),
            Some(24)
        );
        for bad in [
            "",
            "   ",
            "a\u{0007}b",
            "tab\there",
            "new\nline",
            "rtl\u{202E}x",
            "zero\u{200B}w",
            "\u{FEFF}bom",
            &"x".repeat(25),
        ] {
            assert_eq!(normalize(bad), None, "{bad:?}");
        }
        // Markup is ordinary text here; the frontend renders it as text. The classic
        // <img src=x onerror=alert(1)> is 28 characters, so it is too long anyway.
        assert_eq!(normalize("<img src=x onerror=alert(1)>"), None);
        assert_eq!(
            normalize("<svg onload=alert(1)>").as_deref(),
            Some("<svg onload=alert(1)>")
        );
    }

    #[test]
    fn emoji_sequences_count_as_one_character() {
        let family = "👨\u{200D}👩\u{200D}👧\u{200D}👦";
        let rainbow = "🏳\u{FE0F}\u{200D}🌈";
        let toned = "👩🏽\u{200D}👩🏽\u{200D}👧🏽\u{200D}👦🏽";
        for name in [
            family.to_owned(),
            rainbow.to_owned(),
            format!("{rainbow} Kat"),
            "Kat ❤\u{FE0F}".to_owned(),
            "🇺🇸🇯🇵".to_owned(),
            "1\u{FE0F}\u{20E3} Ünïcødé ✓".to_owned(),
            family.repeat(24),
            toned.repeat(24),
        ] {
            assert_eq!(normalize(&name).as_deref(), Some(name.as_str()), "{name:?}");
        }
        assert_eq!(length(&family.repeat(7)), 7);
        assert_eq!(normalize(&family.repeat(25)), None);
        assert_eq!(normalize(&"x".repeat(24)).map(|n| length(&n)), Some(24));
        for bad in [
            "\u{200B}",                // lone zero-width space
            "Ann\u{200B}ie",           // zero-width space inside a name
            "Ann\u{200D}",             // trailing joiner, invisible
            "A\u{200D}nn",             // joiner between letters
            "\u{200D}👩",              // leading joiner
            "Kat\u{FE0F}",             // selector after a letter, invisible
            "rtl\u{202E}x",            // bidi override
            "iso\u{2066}late\u{2069}", // bidi isolates
            "a\u{0007}b",
        ] {
            assert_eq!(normalize(bad), None, "{bad:?}");
        }
        // Stacked combining marks are capped per character.
        assert_eq!(normalize(&format!("e{}", "\u{0301}".repeat(12))), None);
        assert!(normalize(&format!("e{}", "\u{0301}".repeat(3))).is_some());
    }

    #[test]
    fn suffix_keeps_emoji_whole() {
        let family = "👨\u{200D}👩\u{200D}👧\u{200D}👦";
        let name = candidate(&family.repeat(24), 2);
        assert_eq!(length(&name), 24);
        assert!(name.starts_with(&family.repeat(22)));
        assert!(name.ends_with(" 2"));
        assert!(normalize(&name).is_some());
    }

    #[test]
    fn suffixes_fit_the_limit() {
        assert_eq!(candidate("Sam", 1), "Sam");
        assert_eq!(candidate("Sam", 2), "Sam 2");
        let long = "abcdefghijklmnopqrstuvwx";
        assert_eq!(candidate(long, 2), "abcdefghijklmnopqrstuv 2");
        assert_eq!(candidate(long, 10).chars().count(), 24);
        assert_eq!(
            candidate("abcdefghijklmnopqrstu  vw", 2),
            "abcdefghijklmnopqrstu 2"
        );
    }
}
