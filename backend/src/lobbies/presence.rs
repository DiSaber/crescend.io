//! Who is connected right now, so a member who closed their browser without leaving
//! (a "ghost") stops holding up the game and the skip-vote majority.
//!
//! Each open event stream counts as presence for its (lobby, account). When the last
//! stream of an account closes, it is away from that moment; after [`GRACE_SECONDS`]
//! away it is ignored by the game's "everyone has acted" check and by the vote-skip
//! guest count. It stays a member: its points, songs and name stay, and reconnecting
//! makes it count again at once. Accounts never seen on a stream (API clients, a
//! member who has not opened the lobby page yet) count as present.
//!
//! This is memory only: after a restart a ghost counts again until the host removes it
//! (the new process never saw its stream close). The host is never ignored.
use crate::app_state::AppState;
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
    time::Duration,
};

pub const GRACE_SECONDS: i64 = 60;

#[derive(Default)]
struct Entry {
    open: u32,
    /// When the last stream closed (injected clock seconds).
    away_since: i64,
}

// Lobby IDs are random, so entries of different databases (tests) never collide.
static PRESENCE: LazyLock<Mutex<HashMap<(String, i64), Entry>>> = LazyLock::new(Default::default);

fn table() -> std::sync::MutexGuard<'static, HashMap<(String, i64), Entry>> {
    PRESENCE.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Seconds on the wall clock. Presence measures real disconnect time, so it does not
/// use the injected clock that drives token and lobby expiry.
fn wall() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Accounts in `lobby` that have been away for at least the grace period.
pub(crate) fn away(lobby: &str) -> HashSet<i64> {
    let now = wall();
    table()
        .iter()
        .filter(|((id, _), entry)| {
            id == lobby && entry.open == 0 && now - entry.away_since >= GRACE_SECONDS
        })
        .map(|((_, user), _)| *user)
        .collect()
}

/// Drops what is known about an account in a lobby: it left, was removed or joined
/// again, and counts as present until a stream of the new membership closes.
pub(crate) fn forget(lobby: &str, user: i64) {
    table().remove(&(lobby.to_owned(), user));
}

/// Marks an account away for `seconds`, as if its last stream closed that long ago.
#[cfg(test)]
pub(crate) fn mark_away(lobby: &str, user: i64, seconds: i64) {
    table().insert(
        (lobby.to_owned(), user),
        Entry {
            open: 0,
            away_since: wall() - seconds,
        },
    );
}

/// Held by an open event stream. Dropping it (the client went away) starts the
/// grace period; when it passes with no new stream, the lobby settles so a game phase
/// waiting only on this member can close.
pub(crate) struct Presence {
    app: AppState,
    lobby: String,
    user: i64,
}

impl Presence {
    pub(crate) fn connect(app: AppState, lobby: &str, user: i64) -> Self {
        let was_ignored = {
            let mut table = table();
            let entry = table.entry((lobby.to_owned(), user)).or_default();
            let ignored = entry.open == 0
                && entry.away_since != 0
                && wall() - entry.away_since >= GRACE_SECONDS;
            entry.open += 1;
            ignored
        };
        if was_ignored {
            // Counting again changes the skip-vote majority others see.
            settle_later(app.clone(), lobby.to_owned(), Duration::ZERO, None);
        }
        Self {
            app,
            lobby: lobby.to_owned(),
            user,
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let now = wall();
        let away = {
            let mut table = table();
            match table.get_mut(&(self.lobby.clone(), self.user)) {
                Some(entry) => {
                    entry.open = entry.open.saturating_sub(1);
                    if entry.open == 0 {
                        entry.away_since = now;
                    }
                    entry.open == 0
                }
                None => false,
            }
        };
        if away {
            settle_later(
                self.app.clone(),
                self.lobby.clone(),
                Duration::from_secs(GRACE_SECONDS as u64 + 1),
                Some((self.user, now)),
            );
        }
    }
}

/// After `delay`, settle the lobby if `user` is still away since `since` (or always
/// when there is no user to check).
fn settle_later(app: AppState, lobby: String, delay: Duration, check: Option<(i64, i64)>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        tokio::time::sleep(delay).await;
        if let Some((user, since)) = check {
            let still_away = table()
                .get(&(lobby.clone(), user))
                .is_some_and(|entry| entry.open == 0 && entry.away_since == since);
            if !still_away {
                return;
            }
        }
        match app
            .database
            .settle_presence(&lobby, || (app.auth.clock)())
            .await
        {
            Ok(Some(revision)) => app
                .lobby_updates
                .publish(crate::models::lobby::LobbyId::from_trusted(lobby), revision),
            Ok(None) => {}
            Err(_) => eprintln!("Presence settle temporarily unavailable"),
        }
    });
}
