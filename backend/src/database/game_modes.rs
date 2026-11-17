//! Saved custom modes (private to their creator) and the internal mode log.
//!
//! The log is append-only and never served: one row per mode saved and per game
//! started, with the account, time, resolved definition and a hash of its rules and
//! settings, so popular creations can be found later and promoted to presets.
use super::Database;
use crate::{
    auth::AuthenticatedUser,
    game::{GameError, modes::Definition, random_id},
    models::game::GamePrompt,
};
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};
use unicode_segmentation::UnicodeSegmentation;

/// Saved modes one account may keep.
pub const MAX_SAVED_MODES: i64 = 50;

/// A saved mode as stored: its ID, resolved definition and default prompts.
pub(crate) struct SavedMode {
    pub id: String,
    pub definition: Definition,
    pub prompts: Vec<GamePrompt>,
}

#[derive(Clone, Copy)]
pub(crate) enum LogEvent {
    Created,
    Started,
}

/// Identifies a definition by what it plays like: rules and settings, not its name.
pub(crate) fn definition_hash(definition: &Definition) -> String {
    let canonical =
        serde_json::to_string(&(definition.rules, &definition.settings)).expect("definition JSON");
    base16ct::lower::encode_string(&Sha256::digest(canonical.as_bytes()))
}

/// Appends one log row inside the caller's transaction.
pub(crate) async fn log_mode(
    tx: &mut Transaction<'_, Sqlite>,
    account: i64,
    event: LogEvent,
    mode_key: &str,
    definition: &Definition,
    now: i64,
) -> Result<(), GameError> {
    sqlx::query("INSERT INTO game_mode_log(account_id, event, mode_key, definition_hash, definition, logged_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(account)
        .bind(match event {
            LogEvent::Created => "created",
            LogEvent::Started => "started",
        })
        .bind(mode_key)
        .bind(definition_hash(definition))
        .bind(serde_json::to_string(definition).expect("definition JSON"))
        .bind(now)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The definition with a name none of the owner's saved modes or the presets use
/// (compared ignoring case): "Party night", then "Party night 2", "Party night 3".
/// The base is shortened by whole characters so the suffixed name stays within 40.
async fn unique_name(
    tx: &mut Transaction<'_, Sqlite>,
    owner: i64,
    definition: &Definition,
) -> Result<Definition, GameError> {
    let stored: Vec<String> =
        sqlx::query_scalar("SELECT definition FROM game_modes WHERE owner_user_id = ?")
            .bind(owner)
            .fetch_all(&mut **tx)
            .await?;
    let mut taken: Vec<String> = stored
        .iter()
        .filter_map(|d| serde_json::from_str::<Definition>(d).ok())
        .map(|d| d.name.to_lowercase())
        .collect();
    taken.extend(
        crate::game::modes::PRESETS
            .iter()
            .map(|p| (p.definition)().name.to_lowercase()),
    );
    let base = definition.name.trim().to_owned();
    let mut name = base.clone();
    let mut n = 1;
    while taken.contains(&name.to_lowercase()) {
        n += 1;
        let suffix = format!(" {n}");
        let room = crate::game::MAX_MODE_NAME_CHARS - suffix.chars().count();
        // Whole characters only, so an emoji is never split.
        let stem: String = base.graphemes(true).take(room).collect();
        name = format!("{}{suffix}", stem.trim_end());
    }
    Ok(Definition {
        name,
        ..definition.clone()
    })
}

fn parse(id: String, definition: String, prompts: String) -> Result<SavedMode, GameError> {
    let invalid = || GameError::Database(sqlx::Error::Protocol("invalid saved mode".into()));
    Ok(SavedMode {
        id,
        definition: serde_json::from_str(&definition).map_err(|_| invalid())?,
        prompts: serde_json::from_str(&prompts).map_err(|_| invalid())?,
    })
}

async fn account(
    tx: &mut Transaction<'_, Sqlite>,
    user: &AuthenticatedUser,
    now: i64,
) -> Result<(), GameError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
        .bind(user.id)
        .fetch_one(&mut **tx)
        .await?;
    if !exists || now >= user.expires_at {
        return Err(GameError::Unauthorized);
    }
    Ok(())
}

impl Database {
    pub(crate) async fn save_game_mode(
        &self,
        user: &AuthenticatedUser,
        definition: &Definition,
        prompts: &[GamePrompt],
        clock: impl Fn() -> i64,
    ) -> Result<SavedMode, GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        account(&mut tx, user, clock()).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM game_modes WHERE owner_user_id = ?")
                .bind(user.id)
                .fetch_one(&mut *tx)
                .await?;
        if count >= MAX_SAVED_MODES {
            return Err(GameError::ModeLimit);
        }
        let definition = &unique_name(&mut tx, user.id, definition).await?;
        let id = random_id()?;
        sqlx::query("INSERT INTO game_modes(id, owner_user_id, definition, prompts, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(user.id)
            .bind(serde_json::to_string(definition).expect("definition JSON"))
            .bind(serde_json::to_string(prompts).expect("prompts JSON"))
            .bind(clock())
            .execute(&mut *tx)
            .await?;
        log_mode(
            &mut tx,
            user.id,
            LogEvent::Created,
            &id,
            definition,
            clock(),
        )
        .await?;
        tx.commit().await?;
        Ok(SavedMode {
            id,
            definition: definition.clone(),
            prompts: prompts.to_vec(),
        })
    }

    /// The caller's saved modes, oldest first.
    pub(crate) async fn saved_game_modes(
        &self,
        user: &AuthenticatedUser,
        clock: impl Fn() -> i64,
    ) -> Result<Vec<SavedMode>, GameError> {
        let mut tx = self.db_pool.begin().await?;
        account(&mut tx, user, clock()).await?;
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, definition, prompts FROM game_modes WHERE owner_user_id = ? ORDER BY created_at, id",
        )
        .bind(user.id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|(id, definition, prompts)| parse(id, definition, prompts))
            .collect()
    }

    /// One of the caller's saved modes; another account's mode is unavailable.
    pub(crate) async fn saved_game_mode(
        &self,
        user: &AuthenticatedUser,
        id: &str,
    ) -> Result<SavedMode, GameError> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT id, definition, prompts FROM game_modes WHERE id = ? AND owner_user_id = ?",
        )
        .bind(id)
        .bind(user.id)
        .fetch_optional(&self.db_pool)
        .await?;
        let (id, definition, prompts) = row.ok_or(GameError::ModeUnavailable)?;
        parse(id, definition, prompts)
    }

    /// Deletes one of the caller's saved modes. Absent or foreign IDs are no-ops,
    /// so the response never reveals whether another account's mode exists. The
    /// log keeps its rows.
    pub(crate) async fn delete_game_mode(
        &self,
        user: &AuthenticatedUser,
        id: &str,
        clock: impl Fn() -> i64,
    ) -> Result<(), GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        account(&mut tx, user, clock()).await?;
        sqlx::query("DELETE FROM game_modes WHERE id = ? AND owner_user_id = ?")
            .bind(id)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn create_game_mode_tables(&self) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS game_modes (
                id TEXT PRIMARY KEY NOT NULL CHECK(length(id) = 32 AND id NOT GLOB '*[^0-9a-f]*'),
                owner_user_id INTEGER NOT NULL REFERENCES users(id),
                definition TEXT NOT NULL CHECK(json_valid(definition)),
                prompts TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(prompts)),
                created_at INTEGER NOT NULL
            ) STRICT",
            "CREATE INDEX IF NOT EXISTS game_modes_owner ON game_modes(owner_user_id)",
            "CREATE TABLE IF NOT EXISTS game_mode_log (
                id INTEGER PRIMARY KEY NOT NULL,
                account_id INTEGER NOT NULL REFERENCES users(id),
                event TEXT NOT NULL CHECK(event IN ('created', 'started')),
                mode_key TEXT NOT NULL CHECK(length(mode_key) BETWEEN 1 AND 64),
                definition_hash TEXT NOT NULL CHECK(length(definition_hash) = 64),
                definition TEXT NOT NULL CHECK(json_valid(definition)),
                logged_at INTEGER NOT NULL
            ) STRICT",
            "CREATE INDEX IF NOT EXISTS game_mode_log_hash ON game_mode_log(definition_hash, event)",
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await
    }
}
