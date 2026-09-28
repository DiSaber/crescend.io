use super::Database;
use crate::models::lobby::{CurrentLobby, LobbyAllocation, LobbyView, MemberRole, MemberView};
use chrono::{DateTime, Datelike, TimeDelta, Utc};
use sqlx::Acquire;

#[derive(Debug)]
pub enum LobbyError {
    Unauthorized,
    AlreadyInLobby,
    InvalidRequest,
    Unavailable,
    Database(sqlx::Error),
}

impl Database {
    pub(super) async fn create_lobby_tables(&self) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin().await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS lobbies (
                id TEXT PRIMARY KEY NOT NULL CHECK(length(id) = 32 AND id NOT GLOB '*[^0-9a-f]*'),
                join_code TEXT NOT NULL UNIQUE CHECK(length(join_code) = 6 AND join_code NOT GLOB '*[^A-Z0-9]*'),
                owner_user_id INTEGER NOT NULL REFERENCES users(id),
                created_at TEXT NOT NULL CHECK(length(created_at) = 25 AND strftime('%Y-%m-%dT%H:%M:%S+00:00', created_at) IS created_at),
                expires_at TEXT NOT NULL CHECK(length(expires_at) = 25 AND strftime('%Y-%m-%dT%H:%M:%S+00:00', expires_at) IS expires_at AND expires_at > created_at),
                closed_at TEXT CHECK(closed_at IS NULL OR (length(closed_at) = 25 AND strftime('%Y-%m-%dT%H:%M:%S+00:00', closed_at) IS closed_at AND closed_at >= created_at AND closed_at <= expires_at)),
                revision INTEGER NOT NULL CHECK(revision >= 1)
            ) STRICT",
        ).execute(&mut *tx).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS lobby_memberships (
                id TEXT PRIMARY KEY NOT NULL CHECK(length(id) = 32 AND id NOT GLOB '*[^0-9a-f]*'),
                user_id INTEGER NOT NULL UNIQUE REFERENCES users(id),
                lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                joined_at TEXT NOT NULL CHECK(length(joined_at) = 25 AND strftime('%Y-%m-%dT%H:%M:%S+00:00', joined_at) IS joined_at)
            ) STRICT",
        ).execute(&mut *tx).await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS lobbies_expiry ON lobbies(expires_at)")
            .execute(&mut *tx)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS memberships_lobby ON lobby_memberships(lobby_id)")
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    pub async fn create_owned_lobby(
        &self,
        user_id: i64,
        clock: impl Fn() -> i64,
    ) -> Result<CurrentLobby, LobbyError> {
        self.create_owned_lobby_with(user_id, clock, || {
            LobbyAllocation::random().map_err(|_| LobbyError::Unavailable)
        })
        .await
    }

    /// The injected allocator and clock make collisions, expiry, and rollback testable.
    /// Routes never accept these values from clients.
    pub(crate) async fn create_owned_lobby_with(
        &self,
        user_id: i64,
        clock: impl Fn() -> i64,
        mut allocate: impl FnMut() -> Result<LobbyAllocation, LobbyError>,
    ) -> Result<CurrentLobby, LobbyError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = clock();
        let account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        if !account {
            return Err(LobbyError::Unauthorized);
        }
        let created_at = timestamp(now)?;
        let expires_at = created_at
            .checked_add_signed(TimeDelta::days(1))
            .filter(|value| value.year() <= 9999)
            .ok_or(LobbyError::Unavailable)?;
        sqlx::query(
            "DELETE FROM lobby_memberships WHERE user_id = ? AND lobby_id IN
            (SELECT id FROM lobbies WHERE closed_at IS NOT NULL OR expires_at <= ?)",
        )
        .bind(user_id)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        let member: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM lobby_memberships WHERE user_id = ?)")
                .bind(user_id)
                .fetch_one(&mut *tx)
                .await?;
        if member {
            return Err(LobbyError::AlreadyInLobby);
        }

        for _ in 0..8 {
            let candidate = allocate()?;
            // Savepoint rolls back a lobby insert if its membership ID collides.
            // Only the explicit identity/code conflict targets are retryable.
            let mut attempt = tx.begin().await?;
            let inserted = sqlx::query("INSERT INTO lobbies (id, join_code, owner_user_id, created_at, expires_at, revision)
                VALUES (?, ?, ?, ?, ?, 1) ON CONFLICT(id) DO NOTHING ON CONFLICT(join_code) DO NOTHING")
                .bind(candidate.lobby_id.as_str()).bind(&candidate.join_code).bind(user_id)
                .bind(created_at).bind(expires_at).execute(&mut *attempt).await?.rows_affected();
            if inserted == 0 {
                attempt.rollback().await?;
                continue;
            }
            let inserted = sqlx::query(
                "INSERT INTO lobby_memberships (id, user_id, lobby_id, joined_at)
                VALUES (?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
            )
            .bind(candidate.membership_id.as_str())
            .bind(user_id)
            .bind(candidate.lobby_id.as_str())
            .bind(created_at)
            .execute(&mut *attempt)
            .await?
            .rows_affected();
            if inserted == 0 {
                attempt.rollback().await?;
                continue;
            }
            attempt.commit().await?;
            let view = CurrentLobby {
                membership_id: Some(candidate.membership_id),
                lobby: Some(LobbyView {
                    id: candidate.lobby_id,
                    join_code: candidate.join_code,
                    owner_user_id: user_id.to_string(),
                    created_at,
                    expires_at,
                    revision: "1".into(),
                    members: vec![MemberView {
                        user_id: user_id.to_string(),
                        role: MemberRole::for_user(user_id, user_id),
                        joined_at: created_at,
                    }],
                }),
            };
            tx.commit().await?;
            return Ok(view);
        }
        Err(LobbyError::Unavailable)
    }
}

// All lobby timestamps have UTC second precision, including SQLx's +00:00 encoding.
// This canonical representation keeps indexed SQLite TEXT comparisons chronological.
fn timestamp(seconds: i64) -> Result<DateTime<Utc>, LobbyError> {
    DateTime::from_timestamp(seconds, 0)
        .filter(|value| (0..=9999).contains(&value.year()))
        .ok_or(LobbyError::Unavailable)
}

impl From<sqlx::Error> for LobbyError {
    fn from(error: sqlx::Error) -> Self {
        let busy = error
            .as_database_error()
            .and_then(|e| e.code())
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| matches!(code & 255, 5 | 6));
        if busy || matches!(error, sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed) {
            Self::Unavailable
        } else {
            Self::Database(error)
        }
    }
}
