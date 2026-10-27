//! Persistent queue schema and transaction operations.
use super::Database;
use crate::models::{
    queue::{EntryId, QueueEntry, QueueView, SubmissionResponse, VotingView},
    youtube::VideoMetadata,
};
use crate::{
    auth::AuthenticatedUser,
    models::queue::{Revision, timestamp},
    queue::QueueError,
};
use sqlx::{Row, Sqlite, Transaction};

#[cfg(test)]
pub(crate) fn record_commit(lobby: &str, revision: i64) {
    crate::queue::commits()
        .lock()
        .unwrap()
        .insert((lobby.into(), revision), std::time::Instant::now());
}

pub(crate) async fn snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    bound: &BoundQueue,
    user: i64,
    membership: &str,
) -> Result<QueueView, QueueError> {
    let (mode, fixed_count): (String, Option<i64>) =
        sqlx::query_as("SELECT mode, fixed_count FROM lobby_queue_policies WHERE lobby_id = ?")
            .bind(&bound.lobby_id)
            .fetch_one(&mut **tx)
            .await?;
    let guests: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM lobby_memberships WHERE lobby_id = ? AND user_id != ?",
    )
    .bind(&bound.lobby_id)
    .bind(bound.owner)
    .fetch_one(&mut **tx)
    .await?;
    let threshold = if mode == "fixed" {
        fixed_count
    } else if guests == 0 {
        None
    } else {
        Some(guests / 2 + 1)
    };
    let rows = sqlx::query("SELECT e.*, o.position,
        (SELECT count(*) FROM queue_votes v JOIN lobby_memberships m ON m.user_id = v.user_id AND m.lobby_id = e.lobby_id WHERE v.entry_id = e.id AND v.user_id != ?) AS active_vote_count,
        EXISTS(SELECT 1 FROM queue_votes v WHERE v.entry_id = e.id AND v.user_id = ?) AS has_voted
        FROM queue_order o JOIN queue_entries e ON e.id = o.entry_id WHERE o.lobby_id = ? ORDER BY o.position")
        .bind(bound.owner).bind(user).bind(&bound.lobby_id).fetch_all(&mut **tx).await?;
    let mut entries = Vec::with_capacity(rows.len());
    for (position, row) in rows.into_iter().enumerate() {
        if row.try_get::<i64, _>("position")?
            != i64::try_from(position).map_err(|_| QueueError::Unavailable)?
        {
            return Err(QueueError::Database(sqlx::Error::Protocol(
                "noncontiguous queue order".into(),
            )));
        }
        entries.push(QueueEntry {
            id: EntryId::parse(row.try_get("id")?).map_err(|_| QueueError::Unavailable)?,
            video: VideoMetadata {
                video_id: row.try_get("video_id")?,
                title: row.try_get("title")?,
                author_name: row.try_get("author_name")?,
                author_url: row.try_get("author_url")?,
                thumbnail_url: row.try_get("thumbnail_url")?,
                embed_url: row.try_get("embed_url")?,
            },
            submitted_by_user_id: row.try_get::<i64, _>("submitted_by_user_id")?.to_string(),
            submitted_at: row.try_get("submitted_at")?,
            active_vote_count: row.try_get("active_vote_count")?,
            has_voted: user != bound.owner && row.try_get("has_voted")?,
        });
    }
    let mut entries = entries.into_iter();
    Ok(QueueView {
        membership_id: membership.into(),
        lobby_id: bound.lobby_id.clone(),
        revision: bound.revision,
        current: entries.next(),
        upcoming: entries.collect(),
        voting: VotingView {
            mode,
            fixed_count,
            active_guest_count: guests,
            effective_threshold: threshold,
        },
    })
}

async fn receipt(
    tx: &mut Transaction<'_, Sqlite>,
    bound: &BoundQueue,
    user: i64,
    request: &str,
    video: &str,
) -> Result<Option<EntryId>, QueueError> {
    let prior: Option<(String, String)> = sqlx::query_as("SELECT id, video_id FROM queue_entries WHERE lobby_id = ? AND submitted_by_user_id = ? AND request_id = ?")
        .bind(&bound.lobby_id).bind(user).bind(request).fetch_optional(&mut **tx).await?;
    prior
        .map(|(id, old_video)| {
            if old_video != video {
                return Err(QueueError::IdempotencyConflict);
            }
            EntryId::parse(id).map_err(|_| QueueError::Unavailable)
        })
        .transpose()
}

fn submission_response(changed: bool, entry_id: EntryId, queue: QueueView) -> SubmissionResponse {
    let active = queue
        .current
        .iter()
        .chain(queue.upcoming.iter())
        .any(|entry| entry.id == entry_id);
    SubmissionResponse {
        changed,
        entry_id,
        active,
        queue,
    }
}

impl Database {
    pub(crate) async fn move_queue(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        entry: &str,
        request: &crate::models::queue::MoveRequest,
        clock: impl Fn() -> i64,
    ) -> Result<crate::models::queue::MutationResponse, QueueError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        if bound.revision != request.expected_revision {
            return Err(QueueError::StaleRevision);
        }
        let mut entries = ordered_ids(&mut tx, &bound.lobby_id).await?;
        let position = entries
            .iter()
            .position(|id| id == entry)
            .ok_or(QueueError::EntryUnavailable)?;
        if position == 0 {
            return Err(QueueError::NotUpcoming);
        }
        let destination = usize::try_from(request.to_index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .filter(|index| *index < entries.len())
            .ok_or(QueueError::InvalidRequest)?;
        let changed = position != destination;
        if changed {
            let moving = entries.remove(position);
            entries.insert(destination, moving);
            replace_order(&mut tx, &bound.lobby_id, &entries).await?;
            increment(&mut tx, &mut bound).await?;
        }
        let queue = snapshot(&mut tx, &bound, user.id, membership).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        #[cfg(test)]
        if changed {
            record_commit(&bound.lobby_id, bound.revision.value());
        }
        Ok(crate::models::queue::MutationResponse { changed, queue })
    }

    pub(crate) async fn moderate_queue(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        operation: Moderation<'_>,
        clock: impl Fn() -> i64,
    ) -> Result<crate::models::queue::MutationResponse, QueueError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let permission = if matches!(operation, Moderation::Vote(_)) {
            Permission::Guest
        } else {
            Permission::Host
        };
        let mut bound = authorize(&mut tx, user, membership, permission, &clock).await?;
        let changed = match operation {
            Moderation::Vote(entry) => {
                let active: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM queue_order WHERE lobby_id = ? AND entry_id = ?)",
                )
                .bind(&bound.lobby_id)
                .bind(entry)
                .fetch_one(&mut *tx)
                .await?;
                if !active {
                    return Err(QueueError::EntryUnavailable);
                }
                sqlx::query("INSERT INTO queue_votes(entry_id,user_id,created_at) VALUES (?,?,?) ON CONFLICT(entry_id,user_id) DO NOTHING")
                    .bind(entry).bind(user.id).bind(timestamp(clock()).map_err(|_| QueueError::Unavailable)?).execute(&mut *tx).await?.rows_affected() != 0
            }
            Moderation::Policy(policy) => {
                let (mode, count) = policy.parts().map_err(|_| QueueError::InvalidRequest)?;
                let old: (String, Option<i64>) = sqlx::query_as(
                    "SELECT mode,fixed_count FROM lobby_queue_policies WHERE lobby_id = ?",
                )
                .bind(&bound.lobby_id)
                .fetch_one(&mut *tx)
                .await?;
                if old.0 == mode && old.1 == count {
                    false
                } else {
                    sqlx::query("UPDATE lobby_queue_policies SET mode = ?, fixed_count = ? WHERE lobby_id = ?")
                        .bind(mode).bind(count).bind(&bound.lobby_id).execute(&mut *tx).await?;
                    true
                }
            }
        };
        if changed {
            reevaluate(&mut tx, &bound.lobby_id).await?;
            increment(&mut tx, &mut bound).await?;
        }
        let queue = snapshot(&mut tx, &bound, user.id, membership).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        #[cfg(test)]
        if changed {
            record_commit(&bound.lobby_id, bound.revision.value());
        }
        Ok(crate::models::queue::MutationResponse { changed, queue })
    }

    pub(crate) async fn advance_queue(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        expected: &str,
        clock: impl Fn() -> i64,
    ) -> Result<crate::models::queue::MutationResponse, QueueError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        let mut entries = ordered_ids(&mut tx, &bound.lobby_id).await?;
        if entries.first().map(String::as_str) != Some(expected) {
            return Err(QueueError::CurrentChanged);
        }
        entries.remove(0);
        replace_order(&mut tx, &bound.lobby_id, &entries).await?;
        increment(&mut tx, &mut bound).await?;
        let queue = snapshot(&mut tx, &bound, user.id, membership).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        #[cfg(test)]
        record_commit(&bound.lobby_id, bound.revision.value());
        Ok(crate::models::queue::MutationResponse {
            changed: true,
            queue,
        })
    }

    pub(crate) async fn veto_queue(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        entry: &str,
        clock: impl Fn() -> i64,
    ) -> Result<crate::models::queue::MutationResponse, QueueError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        let mut entries = ordered_ids(&mut tx, &bound.lobby_id).await?;
        let previous = entries.len();
        entries.retain(|id| id != entry);
        let changed = previous != entries.len();
        if changed {
            replace_order(&mut tx, &bound.lobby_id, &entries).await?;
            increment(&mut tx, &mut bound).await?;
        }
        let queue = snapshot(&mut tx, &bound, user.id, membership).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        #[cfg(test)]
        if changed {
            record_commit(&bound.lobby_id, bound.revision.value());
        }
        Ok(crate::models::queue::MutationResponse { changed, queue })
    }

    pub(crate) async fn queue_view(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        clock: impl Fn() -> i64,
    ) -> Result<QueueView, QueueError> {
        let mut tx = self.db_pool.begin().await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let view = snapshot(&mut tx, &bound, user.id, membership).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        Ok(view)
    }

    pub(crate) async fn queue_receipt(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        request: &str,
        video: &str,
        clock: impl Fn() -> i64,
    ) -> Result<Option<SubmissionResponse>, QueueError> {
        let mut tx = self.db_pool.begin().await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let response =
            if let Some(entry) = receipt(&mut tx, &bound, user.id, request, video).await? {
                Some(submission_response(
                    false,
                    entry,
                    snapshot(&mut tx, &bound, user.id, membership).await?,
                ))
            } else {
                None
            };
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        Ok(response)
    }

    pub(crate) async fn submit_queue(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        request: &str,
        video: &VideoMetadata,
        clock: impl Fn() -> i64,
        mut allocate: impl FnMut() -> Result<EntryId, QueueError>,
    ) -> Result<SubmissionResponse, QueueError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        if let Some(entry) = receipt(&mut tx, &bound, user.id, request, &video.video_id).await? {
            let view = snapshot(&mut tx, &bound, user.id, membership).await?;
            check_deadlines(&bound, user, clock())?;
            tx.commit().await?;
            return Ok(submission_response(false, entry, view));
        }
        let mut entry = None;
        for _ in 0..8 {
            let id = allocate()?;
            let inserted = sqlx::query("INSERT INTO queue_entries(id,lobby_id,submitted_by_user_id,request_id,video_id,title,author_name,author_url,thumbnail_url,embed_url,submitted_at)
                VALUES (?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING")
                .bind(id.as_str()).bind(&bound.lobby_id).bind(user.id).bind(request).bind(&video.video_id).bind(&video.title)
                .bind(&video.author_name).bind(&video.author_url).bind(&video.thumbnail_url).bind(&video.embed_url)
                .bind(timestamp(clock()).map_err(|_| QueueError::Unavailable)?).execute(&mut *tx).await?.rows_affected();
            if inserted == 1 {
                entry = Some(id);
                break;
            }
        }
        let entry = entry.ok_or(QueueError::Unavailable)?;
        let position: i64 =
            sqlx::query_scalar("SELECT count(*) FROM queue_order WHERE lobby_id = ?")
                .bind(&bound.lobby_id)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query("INSERT INTO queue_order(lobby_id,position,entry_id) VALUES (?,?,?)")
            .bind(&bound.lobby_id)
            .bind(position)
            .bind(entry.as_str())
            .execute(&mut *tx)
            .await?;
        increment(&mut tx, &mut bound).await?;
        let view = snapshot(&mut tx, &bound, user.id, membership).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        #[cfg(test)]
        crate::queue::commits().lock().unwrap().insert(
            (bound.lobby_id.clone(), bound.revision.value()),
            std::time::Instant::now(),
        );
        Ok(submission_response(true, entry, view))
    }
}

pub(crate) enum Moderation<'a> {
    Vote(&'a str),
    Policy(&'a crate::models::queue::PolicyRequest),
}

/// Runs inside the membership/queue writer; owns neither revision nor commit/publication.
pub(crate) async fn reevaluate(
    tx: &mut Transaction<'_, Sqlite>,
    lobby: &str,
) -> Result<(), sqlx::Error> {
    let (mode,fixed,guests): (String,Option<i64>,i64) = sqlx::query_as("SELECT p.mode,p.fixed_count,
        (SELECT count(*) FROM lobby_memberships m WHERE m.lobby_id = l.id AND m.user_id != l.owner_user_id)
        FROM lobby_queue_policies p JOIN lobbies l ON l.id = p.lobby_id WHERE l.id = ?")
        .bind(lobby).fetch_one(&mut **tx).await?;
    if guests == 0 {
        return Ok(());
    }
    let threshold = if mode == "fixed" {
        fixed.ok_or_else(|| sqlx::Error::Protocol("missing fixed policy".into()))?
    } else {
        guests / 2 + 1
    };
    let qualifying: Vec<String> = sqlx::query_scalar("SELECT o.entry_id FROM queue_order o JOIN lobbies l ON l.id=o.lobby_id
        WHERE o.lobby_id = ? AND (SELECT count(*) FROM queue_votes v JOIN lobby_memberships m ON m.user_id=v.user_id AND m.lobby_id=l.id
            WHERE v.entry_id=o.entry_id AND v.user_id != l.owner_user_id) >= ?")
        .bind(lobby).bind(threshold).fetch_all(&mut **tx).await?;
    if !qualifying.is_empty() {
        let mut ids = ordered_ids(tx, lobby).await?;
        ids.retain(|entry| !qualifying.contains(entry));
        replace_order(tx, lobby, &ids).await?;
    }
    Ok(())
}

pub(crate) async fn ordered_ids(
    tx: &mut Transaction<'_, Sqlite>,
    lobby: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT entry_id FROM queue_order WHERE lobby_id = ? ORDER BY position")
        .bind(lobby)
        .fetch_all(&mut **tx)
        .await
}

pub(crate) async fn replace_order(
    tx: &mut Transaction<'_, Sqlite>,
    lobby: &str,
    entries: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM queue_order WHERE lobby_id = ?")
        .bind(lobby)
        .execute(&mut **tx)
        .await?;
    for (position, id) in entries.iter().enumerate() {
        sqlx::query("INSERT INTO queue_order(lobby_id,position,entry_id) VALUES (?,?,?)")
            .bind(lobby)
            .bind(
                i64::try_from(position)
                    .map_err(|_| sqlx::Error::Protocol("order exhausted".into()))?,
            )
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

pub(crate) struct BoundQueue {
    pub lobby_id: String,
    pub owner: i64,
    pub revision: Revision,
    pub expires_at: i64,
}

#[derive(Clone, Copy)]
pub(crate) enum Permission {
    Member,
    Host,
    Guest,
}

pub(crate) async fn authorize(
    tx: &mut Transaction<'_, Sqlite>,
    user: &AuthenticatedUser,
    membership: &str,
    permission: Permission,
    clock: &impl Fn() -> i64,
) -> Result<BoundQueue, QueueError> {
    let account: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
        .bind(user.id)
        .fetch_one(&mut **tx)
        .await?;
    if !account || clock() >= user.expires_at {
        return Err(QueueError::Unauthorized);
    }
    let row = sqlx::query("SELECT l.id, l.owner_user_id, l.revision, l.expires_at FROM lobby_memberships m JOIN lobbies l ON l.id = m.lobby_id WHERE m.id = ? AND m.user_id = ? AND l.closed_at IS NULL")
        .bind(membership).bind(user.id).fetch_optional(&mut **tx).await?
        .ok_or(QueueError::MembershipForbidden)?;
    let expires_at: chrono::DateTime<chrono::Utc> = row.try_get("expires_at")?;
    if clock() >= expires_at.timestamp() {
        return Err(QueueError::MembershipForbidden);
    }
    let owner = row.try_get("owner_user_id")?;
    match permission {
        Permission::Host if owner != user.id => return Err(QueueError::HostRequired),
        Permission::Guest if owner == user.id => return Err(QueueError::GuestRequired),
        _ => (),
    }
    Ok(BoundQueue {
        lobby_id: row.try_get("id")?,
        owner,
        revision: Revision::new(row.try_get("revision")?).map_err(|_| QueueError::Unavailable)?,
        expires_at: expires_at.timestamp(),
    })
}

pub(crate) fn check_deadlines(
    bound: &BoundQueue,
    user: &AuthenticatedUser,
    now: i64,
) -> Result<(), QueueError> {
    if now >= user.expires_at {
        return Err(QueueError::Unauthorized);
    }
    if now >= bound.expires_at {
        return Err(QueueError::MembershipForbidden);
    }
    timestamp(now).map_err(|_| QueueError::Unavailable)?;
    Ok(())
}

pub(crate) async fn increment(
    tx: &mut Transaction<'_, Sqlite>,
    bound: &mut BoundQueue,
) -> Result<(), QueueError> {
    bound.revision = bound.revision.next().map_err(|_| QueueError::Unavailable)?;
    sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
        .bind(bound.revision.value())
        .bind(&bound.lobby_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

impl Database {
    pub(super) async fn create_queue_tables(&self) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS lobby_queue_policies (
                lobby_id TEXT PRIMARY KEY NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                mode TEXT NOT NULL DEFAULT 'majority',
                fixed_count INTEGER,
                CHECK ((mode = 'majority' AND fixed_count IS NULL) OR
                    (mode = 'fixed' AND fixed_count BETWEEN 1 AND 2147483647 AND fixed_count IS NOT NULL))
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS queue_entries (
                id TEXT PRIMARY KEY NOT NULL CHECK(length(id) = 32 AND id NOT GLOB '*[^0-9a-f]*'),
                lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                submitted_by_user_id INTEGER NOT NULL REFERENCES users(id),
                request_id TEXT NOT NULL CHECK(length(request_id) = 32 AND request_id NOT GLOB '*[^0-9a-f]*'),
                video_id TEXT NOT NULL,
                title TEXT NOT NULL, author_name TEXT NOT NULL, author_url TEXT NOT NULL,
                thumbnail_url TEXT NOT NULL, embed_url TEXT NOT NULL,
                submitted_at TEXT NOT NULL CHECK(length(submitted_at) = 25 AND strftime('%Y-%m-%dT%H:%M:%S+00:00', submitted_at) IS submitted_at),
                UNIQUE(lobby_id, submitted_by_user_id, request_id), UNIQUE(lobby_id, id)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS queue_order (
                lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                position INTEGER NOT NULL CHECK(position >= 0),
                entry_id TEXT NOT NULL UNIQUE,
                PRIMARY KEY(lobby_id, position),
                FOREIGN KEY(lobby_id, entry_id) REFERENCES queue_entries(lobby_id, id) ON DELETE CASCADE
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS queue_votes (
                entry_id TEXT NOT NULL REFERENCES queue_entries(id) ON DELETE CASCADE,
                user_id INTEGER NOT NULL REFERENCES users(id),
                created_at TEXT NOT NULL CHECK(length(created_at) = 25 AND strftime('%Y-%m-%dT%H:%M:%S+00:00', created_at) IS created_at),
                PRIMARY KEY(entry_id, user_id)
            ) STRICT",
            "INSERT INTO lobby_queue_policies(lobby_id) SELECT id FROM lobbies WHERE closed_at IS NULL ON CONFLICT(lobby_id) DO NOTHING",
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await
    }
}
