//! Per-lobby display names. A name belongs to one (lobby, account) pair, so the same
//! person can use different names in different lobbies. Rows outlive the membership
//! (they go with the lobby), which keeps departed players named on the scoreboard and
//! in "added by", and keeps their name reserved if they come back.
use super::Database;
use chrono::Utc;
use sqlx::SqliteConnection;
use std::collections::HashMap;

pub const MAX_CHARS: usize = 24;
/// Used when a client sends no nickname and the account has never chosen one.
pub const DEFAULT_GUEST: &str = "Player";
pub const DEFAULT_HOST: &str = "Host";

/// Trims, collapses inner whitespace runs to one space and checks the rules:
/// 1-24 characters, no control or invisible formatting characters.
pub fn normalize(raw: &str) -> Option<String> {
    if raw.chars().any(forbidden) {
        return None;
    }
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = name.chars().count();
    (1..=MAX_CHARS).contains(&count).then_some(name)
}

/// Control characters (Cc) plus the zero-width and bidirectional formatting characters
/// that can make two names look alike or reorder surrounding text.
fn forbidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
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
    let keep = MAX_CHARS - suffix.chars().count();
    let stem: String = base.chars().take(keep).collect();
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
        // New tables only: existing users/membership rows are not rebuilt.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS lobby_nicknames (
                lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                user_id INTEGER NOT NULL REFERENCES users(id),
                nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 24),
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
                nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 24),
                updated_at INTEGER NOT NULL
            ) STRICT",
        )
        .execute(&self.db_pool)
        .await?;
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
