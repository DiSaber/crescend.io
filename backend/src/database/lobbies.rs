use super::Database;
use crate::models::lobby::{
    CurrentLobby, LobbyAllocation, LobbyId, LobbyView, MemberRole, MemberView, MembershipId,
};
use chrono::{DateTime, Datelike, TimeDelta, Utc};
use sqlx::{Acquire, Row};

#[derive(Debug)]
pub enum LobbyError {
    Unauthorized,
    MembershipForbidden,
    AlreadyInLobby,
    InvalidRequest,
    InvalidJoinCode,
    LobbyUnavailable,
    Unavailable,
    Database(sqlx::Error),
}

#[derive(sqlx::FromRow)]
pub(crate) struct StreamMembership {
    pub lobby_id: LobbyId,
    pub revision: i64,
    pub expires_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Database {
    pub(crate) async fn stream_membership(
        &self,
        user_id: i64,
        membership_id: &str,
    ) -> Result<Option<StreamMembership>, LobbyError> {
        // One statement is one coherent snapshot; no roster or transaction is
        // retained by the transport while it waits for a consumer.
        Ok(sqlx::query_as("SELECT l.id AS lobby_id, l.revision, l.expires_at, l.closed_at FROM lobby_memberships m JOIN lobbies l ON l.id = m.lobby_id JOIN users u ON u.id = m.user_id WHERE m.id = ? AND m.user_id = ?")
            .bind(membership_id).bind(user_id).fetch_optional(&self.db_pool).await?)
    }

    pub(crate) async fn require_lobby_account(&self, user_id: i64) -> Result<(), LobbyError> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(user_id)
            .fetch_one(&self.db_pool)
            .await?;
        if exists {
            Ok(())
        } else {
            Err(LobbyError::Unauthorized)
        }
    }
    pub async fn current_lobby(
        &self,
        user_id: i64,
        clock: impl Fn() -> i64,
    ) -> Result<CurrentLobby, LobbyError> {
        // A deferred read transaction keeps membership, revision and roster in
        // one SQLite snapshot without reserving the writer.
        let mut tx = self.db_pool.begin().await?;
        let account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        if !account {
            return Err(LobbyError::Unauthorized);
        }
        let now = timestamp(clock())?;
        let row = sqlx::query("SELECT m.id AS membership_id, l.id, l.join_code, l.owner_user_id, l.created_at, l.expires_at, l.revision FROM lobby_memberships m JOIN lobbies l ON l.id = m.lobby_id WHERE m.user_id = ? AND l.closed_at IS NULL AND l.expires_at > ?")
            .bind(user_id).bind(now).fetch_optional(&mut *tx).await?;
        let mut view = CurrentLobby {
            membership_id: None,
            lobby: None,
        };
        if let Some(row) = row {
            let id: LobbyId = row.try_get("id")?;
            let owner: i64 = row.try_get("owner_user_id")?;
            let revision: i64 = row.try_get("revision")?;
            let members: Vec<(i64, DateTime<Utc>)> = sqlx::query_as("SELECT user_id, joined_at FROM lobby_memberships WHERE lobby_id = ? ORDER BY joined_at, user_id")
                .bind(id.as_str()).fetch_all(&mut *tx).await?;
            view = CurrentLobby {
                membership_id: Some(row.try_get("membership_id")?),
                lobby: Some(LobbyView {
                    id,
                    join_code: row.try_get("join_code")?,
                    owner_user_id: owner.to_string(),
                    created_at: row.try_get("created_at")?,
                    expires_at: row.try_get("expires_at")?,
                    revision: revision.to_string(),
                    members: members
                        .into_iter()
                        .map(|(id, joined_at)| MemberView {
                            user_id: id.to_string(),
                            role: MemberRole::for_user(id, owner),
                            joined_at,
                        })
                        .collect(),
                }),
            };
        }
        tx.commit().await?;
        // Time may advance while reading or releasing the snapshot. Never
        // return an active view at or after its deadline, even before cleanup.
        if let Some(lobby) = &view.lobby {
            if lobby.expires_at <= timestamp(clock())? {
                return Ok(CurrentLobby {
                    membership_id: None,
                    lobby: None,
                });
            }
        }
        Ok(view)
    }

    #[cfg(test)]
    pub async fn join_lobby(
        &self,
        user_id: i64,
        code: &str,
        clock: impl Fn() -> i64,
    ) -> Result<CurrentLobby, LobbyError> {
        self.join_lobby_with(user_id, code, clock, || {
            MembershipId::random().map_err(|_| LobbyError::Unavailable)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn join_lobby_with(
        &self,
        user_id: i64,
        code: &str,
        clock: impl Fn() -> i64,
        allocate: impl FnMut() -> Result<MembershipId, LobbyError>,
    ) -> Result<CurrentLobby, LobbyError> {
        self.join_lobby_outcome_with(user_id, code, clock, allocate)
            .await
            .map(|(view, _)| view)
    }

    pub(crate) async fn join_lobby_outcome_with(
        &self,
        user_id: i64,
        code: &str,
        clock: impl Fn() -> i64,
        mut allocate: impl FnMut() -> Result<MembershipId, LobbyError>,
    ) -> Result<(CurrentLobby, bool), LobbyError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = timestamp(clock())?;
        let account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        if !account {
            return Err(LobbyError::Unauthorized);
        }
        let lobby = sqlx::query("SELECT id, join_code, owner_user_id, created_at, expires_at, revision FROM lobbies WHERE join_code = ? AND closed_at IS NULL AND expires_at > ?")
            .bind(code).bind(now).fetch_optional(&mut *tx).await?
            .ok_or(LobbyError::LobbyUnavailable)?;
        let lobby_id: LobbyId = lobby.try_get("id")?;
        let owner: i64 = lobby.try_get("owner_user_id")?;
        let mut revision: i64 = lobby.try_get("revision")?;
        sqlx::query("DELETE FROM lobby_memberships WHERE user_id = ? AND lobby_id IN (SELECT id FROM lobbies WHERE closed_at IS NOT NULL OR expires_at <= ?)")
            .bind(user_id).bind(now).execute(&mut *tx).await?;
        let existing: Option<(MembershipId, LobbyId)> =
            sqlx::query_as("SELECT id, lobby_id FROM lobby_memberships WHERE user_id = ?")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let changed = existing.is_none();
        let membership_id = if let Some((id, target)) = existing {
            if target != lobby_id {
                return Err(LobbyError::AlreadyInLobby);
            }
            id
        } else {
            let mut allocated = None;
            for _ in 0..8 {
                let id = allocate()?;
                let inserted = sqlx::query("INSERT INTO lobby_memberships (id, user_id, lobby_id, joined_at) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO NOTHING")
                    .bind(id.as_str()).bind(user_id).bind(lobby_id.as_str()).bind(now)
                    .execute(&mut *tx).await?.rows_affected();
                if inserted == 1 {
                    allocated = Some(id);
                    break;
                }
            }
            let id = allocated.ok_or(LobbyError::Unavailable)?;
            revision = revision.checked_add(1).ok_or(LobbyError::Unavailable)?;
            sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
                .bind(revision)
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
            id
        };
        let rows: Vec<(i64, DateTime<Utc>)> = sqlx::query_as("SELECT user_id, joined_at FROM lobby_memberships WHERE lobby_id = ? ORDER BY joined_at, user_id")
            .bind(lobby_id.as_str()).fetch_all(&mut *tx).await?;
        let view = CurrentLobby {
            membership_id: Some(membership_id),
            lobby: Some(LobbyView {
                id: lobby_id,
                join_code: lobby.try_get("join_code")?,
                owner_user_id: owner.to_string(),
                created_at: lobby.try_get("created_at")?,
                expires_at: lobby.try_get("expires_at")?,
                revision: revision.to_string(),
                members: rows
                    .into_iter()
                    .map(|(id, joined_at)| MemberView {
                        user_id: id.to_string(),
                        role: MemberRole::for_user(id, owner),
                        joined_at,
                    })
                    .collect(),
            }),
        };
        tx.commit().await?;
        Ok((view, changed))
    }

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
