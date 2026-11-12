use super::Database;
use crate::models::lobby::{
    CurrentLobby, LobbyAllocation, LobbyId, LobbyView, MemberRole, MemberView, MembershipId,
};
use chrono::{DateTime, Datelike, TimeDelta, Utc};
use sqlx::{Acquire, Row};

#[derive(Debug)]
pub enum LobbyError {
    Unauthorized,
    GoogleAccountRequired,
    InvalidNickname,
    MembershipForbidden,
    /// Only the host can do this.
    HostRequired,
    CannotRemoveHost,
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
    pub expires_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct BoundStreamState {
    pub revision: i64,
    pub closed_at: Option<DateTime<Utc>>,
    pub member_exists: bool,
    /// The host removed this membership (`remove_member`).
    pub removed: bool,
}

/// What `remove_member` did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Removal {
    /// The member is gone; the lobby moved to this revision.
    Removed(LobbyId, i64),
    /// They had already left: nothing changed.
    Absent,
}

impl Database {
    pub(crate) async fn leave_lobby(
        &self,
        user_id: i64,
        membership_id: &str,
        clock: impl Fn() -> i64,
    ) -> Result<Option<(LobbyId, i64)>, LobbyError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = timestamp(clock())?;
        let account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        if !account {
            return Err(LobbyError::Unauthorized);
        }
        let row = sqlx::query("SELECT m.user_id, l.id, l.owner_user_id, l.expires_at, l.closed_at, l.revision FROM lobby_memberships m JOIN lobbies l ON l.id = m.lobby_id WHERE m.id = ?")
            .bind(membership_id).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        if row.try_get::<i64, _>("user_id")? != user_id {
            return Err(LobbyError::MembershipForbidden);
        }
        let lobby_id: LobbyId = row.try_get("id")?;
        let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
        let closed_at: Option<DateTime<Utc>> = row.try_get("closed_at")?;
        if closed_at.is_some() || expires_at <= now {
            // Release only this stale slot. Logical expiry/closure already ended
            // access, so this cleanup must not publish or increment revision.
            sqlx::query("DELETE FROM lobby_memberships WHERE id = ? AND user_id = ?")
                .bind(membership_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(None);
        }
        let revision = row
            .try_get::<i64, _>("revision")?
            .checked_add(1)
            .ok_or(LobbyError::Unavailable)?;
        if row.try_get::<i64, _>("owner_user_id")? == user_id {
            sqlx::query("UPDATE lobbies SET closed_at = ?, revision = ? WHERE id = ?")
                .bind(now)
                .bind(revision)
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM lobby_memberships WHERE lobby_id = ?")
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM queue_entries WHERE lobby_id = ?")
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM games WHERE lobby_id = ?")
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
            // Files go with the rows: the route sweeps this lobby's folder after commit.
            sqlx::query("DELETE FROM game_images WHERE lobby_id = ?")
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM lobby_queue_policies WHERE lobby_id = ?")
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
        } else {
            depart(
                &mut tx,
                lobby_id.as_str(),
                membership_id,
                user_id,
                revision,
                clock(),
            )
            .await?;
        }
        tx.commit().await?;
        #[cfg(test)]
        super::queue::record_commit(lobby_id.as_str(), revision);
        Ok(Some((lobby_id, revision)))
    }

    /// The host removes another member, typically a "ghost" who closed their browser
    /// without leaving. It works like that member leaving (votes stop counting, points
    /// stay and show as left, a game phase waiting on them can close) and their open
    /// streams end with reason `removed`. They can rejoin with the code.
    pub(crate) async fn remove_member(
        &self,
        host: i64,
        host_membership: &str,
        target: i64,
        clock: impl Fn() -> i64,
    ) -> Result<Removal, LobbyError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = timestamp(clock())?;
        let account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
            .bind(host)
            .fetch_one(&mut *tx)
            .await?;
        if !account {
            return Err(LobbyError::Unauthorized);
        }
        let lobby: Option<(LobbyId, i64, i64)> = sqlx::query_as("SELECT l.id, l.owner_user_id, l.revision FROM lobby_memberships m JOIN lobbies l ON l.id = m.lobby_id WHERE m.id = ? AND m.user_id = ? AND l.closed_at IS NULL AND l.expires_at > ?")
            .bind(host_membership).bind(host).bind(now).fetch_optional(&mut *tx).await?;
        let (lobby_id, owner, revision) = lobby.ok_or(LobbyError::MembershipForbidden)?;
        if owner != host {
            return Err(LobbyError::HostRequired);
        }
        if target == host {
            return Err(LobbyError::CannotRemoveHost);
        }
        let membership: Option<String> = sqlx::query_scalar(
            "SELECT id FROM lobby_memberships WHERE lobby_id = ? AND user_id = ?",
        )
        .bind(lobby_id.as_str())
        .bind(target)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(membership) = membership else {
            tx.commit().await?;
            return Ok(Removal::Absent);
        };
        let revision = revision.checked_add(1).ok_or(LobbyError::Unavailable)?;
        sqlx::query("INSERT INTO lobby_removals (membership_id, lobby_id) VALUES (?, ?)")
            .bind(&membership)
            .bind(lobby_id.as_str())
            .execute(&mut *tx)
            .await?;
        depart(
            &mut tx,
            lobby_id.as_str(),
            &membership,
            target,
            revision,
            clock(),
        )
        .await?;
        tx.commit().await?;
        #[cfg(test)]
        super::queue::record_commit(lobby_id.as_str(), revision);
        Ok(Removal::Removed(lobby_id, revision))
    }

    /// Someone's minute away ran out (or they came back): recount the skip majority
    /// and let a game phase that now has everyone's input close. Returns the new
    /// revision so the caller can notify, or None when the lobby is gone.
    pub(crate) async fn settle_presence(
        &self,
        lobby: &str,
        clock: impl Fn() -> i64,
    ) -> Result<Option<i64>, LobbyError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = timestamp(clock())?;
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM lobbies WHERE id = ? AND closed_at IS NULL AND expires_at > ?",
        )
        .bind(lobby)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(revision) = revision else {
            return Ok(None);
        };
        let revision = revision.checked_add(1).ok_or(LobbyError::Unavailable)?;
        super::queue::reevaluate(&mut tx, lobby).await?;
        super::game::settle_after_leave(&mut tx, lobby, clock())
            .await
            .map_err(game_error)?;
        sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
            .bind(revision)
            .bind(lobby)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        #[cfg(test)]
        super::queue::record_commit(lobby, revision);
        Ok(Some(revision))
    }

    /// Whether an active lobby has this (normalized) code. Guest sign-in checks it so a
    /// wrong code never creates an account.
    pub(crate) async fn lobby_code_active(
        &self,
        code: &str,
        clock: impl Fn() -> i64,
    ) -> Result<bool, LobbyError> {
        let now = timestamp(clock())?;
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM lobbies WHERE join_code = ? AND closed_at IS NULL AND expires_at > ?)",
        )
        .bind(code)
        .bind(now)
        .fetch_one(&self.db_pool)
        .await?)
    }

    pub(crate) async fn bound_stream_state(
        &self,
        user_id: i64,
        membership_id: &str,
        lobby_id: &LobbyId,
    ) -> Result<Option<BoundStreamState>, LobbyError> {
        // Keep closure and generation existence in one snapshot. Closure removes
        // memberships, but the retained lobby supplies the terminal reason.
        Ok(sqlx::query_as("SELECT l.revision, l.closed_at, EXISTS(SELECT 1 FROM lobby_memberships m JOIN users u ON u.id = m.user_id WHERE m.id = ? AND m.user_id = ? AND m.lobby_id = l.id) AS member_exists, EXISTS(SELECT 1 FROM lobby_removals r WHERE r.membership_id = ?) AS removed FROM lobbies l WHERE l.id = ?")
            .bind(membership_id).bind(user_id).bind(membership_id).bind(lobby_id.as_str())
            .fetch_optional(&self.db_pool).await?)
    }

    pub(crate) async fn stream_membership(
        &self,
        user_id: i64,
        membership_id: &str,
    ) -> Result<Option<StreamMembership>, LobbyError> {
        // One statement is one coherent snapshot; no roster or transaction is
        // retained by the transport while it waits for a consumer.
        Ok(sqlx::query_as("SELECT l.id AS lobby_id, l.expires_at, l.closed_at FROM lobby_memberships m JOIN lobbies l ON l.id = m.lobby_id JOIN users u ON u.id = m.user_id WHERE m.id = ? AND m.user_id = ?")
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
            let members = roster(&mut tx, &id, owner).await?;
            view = CurrentLobby {
                membership_id: Some(row.try_get("membership_id")?),
                lobby: Some(LobbyView {
                    id,
                    join_code: row.try_get("join_code")?,
                    owner_user_id: owner.to_string(),
                    created_at: row.try_get("created_at")?,
                    expires_at: row.try_get("expires_at")?,
                    revision: revision.to_string(),
                    members,
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
            .map(|(view, _, _)| view)
    }

    pub(crate) async fn join_lobby_outcome_with(
        &self,
        user_id: i64,
        code: &str,
        clock: impl Fn() -> i64,
        allocate: impl FnMut() -> Result<MembershipId, LobbyError>,
    ) -> Result<(CurrentLobby, bool, Option<(LobbyId, i64)>), LobbyError> {
        self.join_lobby_named(user_id, code, None, clock, allocate)
            .await
    }

    /// Joins with an already normalized nickname, or none: keep this lobby's name, else
    /// use the account's last one. Renaming inside the same lobby counts as a change.
    pub(crate) async fn join_lobby_named(
        &self,
        user_id: i64,
        code: &str,
        nickname: Option<&str>,
        clock: impl Fn() -> i64,
        mut allocate: impl FnMut() -> Result<MembershipId, LobbyError>,
    ) -> Result<(CurrentLobby, bool, Option<(LobbyId, i64)>), LobbyError> {
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
        let mut departed = None;
        // The host opening their own join link (a second tab, say) is never renamed:
        // the join page tells them they are hosting instead.
        let hosting_here = existing
            .as_ref()
            .is_some_and(|(_, target)| *target == lobby_id && owner == user_id);
        let nickname = if hosting_here { None } else { nickname };
        let existing = if let Some((id, target)) = existing {
            if target == lobby_id {
                Some(id)
            } else {
                let (old_owner, old_revision): (i64, i64) =
                    sqlx::query_as("SELECT owner_user_id, revision FROM lobbies WHERE id = ?")
                        .bind(target.as_str())
                        .fetch_one(&mut *tx)
                        .await?;
                if old_owner == user_id {
                    return Err(LobbyError::AlreadyInLobby);
                }
                let old_revision = old_revision.checked_add(1).ok_or(LobbyError::Unavailable)?;
                sqlx::query("DELETE FROM lobby_memberships WHERE id = ?")
                    .bind(id.as_str())
                    .execute(&mut *tx)
                    .await?;
                super::queue::reevaluate(&mut tx, target.as_str()).await?;
                sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
                    .bind(old_revision)
                    .bind(target.as_str())
                    .execute(&mut *tx)
                    .await?;
                crate::lobbies::presence::forget(target.as_str(), user_id);
                departed = Some((target, old_revision));
                None
            }
        } else {
            None
        };
        let (wanted, explicit) = super::nicknames::resolve(
            &mut tx,
            lobby_id.as_str(),
            user_id,
            nickname,
            super::nicknames::DEFAULT_GUEST,
        )
        .await?;
        let (_, renamed) =
            super::nicknames::assign(&mut tx, lobby_id.as_str(), user_id, &wanted, explicit)
                .await?;
        let changed = existing.is_none() || renamed;
        let membership_id = if let Some(id) = existing {
            if renamed {
                revision = revision.checked_add(1).ok_or(LobbyError::Unavailable)?;
                sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
                    .bind(revision)
                    .bind(lobby_id.as_str())
                    .execute(&mut *tx)
                    .await?;
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
            crate::lobbies::presence::forget(lobby_id.as_str(), user_id);
            super::queue::reevaluate(&mut tx, lobby_id.as_str()).await?;
            revision = revision.checked_add(1).ok_or(LobbyError::Unavailable)?;
            sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
                .bind(revision)
                .bind(lobby_id.as_str())
                .execute(&mut *tx)
                .await?;
            id
        };
        let members = roster(&mut tx, &lobby_id, owner).await?;
        let view = CurrentLobby {
            membership_id: Some(membership_id),
            lobby: Some(LobbyView {
                id: lobby_id,
                join_code: lobby.try_get("join_code")?,
                owner_user_id: owner.to_string(),
                created_at: lobby.try_get("created_at")?,
                expires_at: lobby.try_get("expires_at")?,
                revision: revision.to_string(),
                members,
            }),
        };
        tx.commit().await?;
        #[cfg(test)]
        if changed {
            super::queue::record_commit(view.lobby.as_ref().unwrap().id.as_str(), revision);
        }
        #[cfg(test)]
        if let Some((id, revision)) = &departed {
            super::queue::record_commit(id.as_str(), *revision);
        }
        Ok((view, changed, departed))
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
        // Memberships the host removed, so their streams can say why they ended.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS lobby_removals (
                membership_id TEXT PRIMARY KEY NOT NULL CHECK(length(membership_id) = 32),
                lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE
            ) STRICT",
        )
        .execute(&mut *tx)
        .await?;
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
        allocate: impl FnMut() -> Result<LobbyAllocation, LobbyError>,
    ) -> Result<CurrentLobby, LobbyError> {
        self.create_named_lobby_with(user_id, None, clock, allocate)
            .await
    }

    pub async fn create_named_lobby(
        &self,
        user_id: i64,
        nickname: Option<&str>,
        clock: impl Fn() -> i64,
    ) -> Result<CurrentLobby, LobbyError> {
        self.create_named_lobby_with(user_id, nickname, clock, || {
            LobbyAllocation::random().map_err(|_| LobbyError::Unavailable)
        })
        .await
    }

    pub(crate) async fn create_named_lobby_with(
        &self,
        user_id: i64,
        nickname: Option<&str>,
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
            sqlx::query("INSERT INTO lobby_queue_policies(lobby_id) VALUES (?)")
                .bind(candidate.lobby_id.as_str())
                .execute(&mut *attempt)
                .await?;
            let (wanted, explicit) = super::nicknames::resolve(
                &mut attempt,
                candidate.lobby_id.as_str(),
                user_id,
                nickname,
                super::nicknames::DEFAULT_HOST,
            )
            .await?;
            let (name, _) = super::nicknames::assign(
                &mut attempt,
                candidate.lobby_id.as_str(),
                user_id,
                &wanted,
                explicit,
            )
            .await?;
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
                        nickname: name,
                        joined_at: created_at,
                        away: false,
                    }],
                }),
            };
            tx.commit().await?;
            return Ok(view);
        }
        Err(LobbyError::Unavailable)
    }
}

/// Ends one ordinary (non-host) membership inside the caller's writer transaction:
/// the queue recounts votes, a game phase waiting on them can close, and the lobby
/// moves to `revision`.
async fn depart(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    lobby: &str,
    membership: &str,
    user: i64,
    revision: i64,
    now: i64,
) -> Result<(), LobbyError> {
    sqlx::query("DELETE FROM lobby_memberships WHERE id = ? AND user_id = ?")
        .bind(membership)
        .bind(user)
        .execute(&mut **tx)
        .await?;
    crate::lobbies::presence::forget(lobby, user);
    super::queue::reevaluate(tx, lobby).await?;
    super::game::settle_after_leave(tx, lobby, now)
        .await
        .map_err(game_error)?;
    sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
        .bind(revision)
        .bind(lobby)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn game_error(error: crate::game::GameError) -> LobbyError {
    match error {
        crate::game::GameError::Database(error) => LobbyError::from(error),
        _ => LobbyError::Unavailable,
    }
}

/// Current members in join order, each with their name in this lobby and whether the
/// game is skipping them for presence (`away`).
async fn roster(
    conn: &mut sqlx::SqliteConnection,
    lobby: &LobbyId,
    owner: i64,
) -> Result<Vec<MemberView>, sqlx::Error> {
    let rows: Vec<(i64, DateTime<Utc>)> = sqlx::query_as("SELECT user_id, joined_at FROM lobby_memberships WHERE lobby_id = ? ORDER BY joined_at, user_id")
        .bind(lobby.as_str()).fetch_all(&mut *conn).await?;
    let names = super::nicknames::lobby_names(&mut *conn, lobby.as_str()).await?;
    let away = crate::lobbies::presence::away(lobby.as_str());
    Ok(rows
        .into_iter()
        .map(|(id, joined_at)| MemberView {
            user_id: id.to_string(),
            role: MemberRole::for_user(id, owner),
            nickname: super::nicknames::name_of(&names, id),
            joined_at,
            away: id != owner && away.contains(&id),
        })
        .collect())
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
