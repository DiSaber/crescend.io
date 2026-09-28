use crate::{
    app_state::AppState, auth::AuthenticatedUser, database::lobbies::StreamMembership,
    models::lobby::LobbyId,
};
use axum::response::sse::Event;
use futures_util::{Stream, stream};
use std::{convert::Infallible, time::Duration};
use tokio::{sync::broadcast, time::Instant};

#[derive(Clone)]
pub struct LobbyUpdates(broadcast::Sender<(LobbyId, i64)>);

impl Default for LobbyUpdates {
    fn default() -> Self {
        Self(broadcast::channel(256).0)
    }
}

impl LobbyUpdates {
    pub fn publish(&self, lobby: LobbyId, revision: i64) {
        // A missed notification never changes the committed database result.
        let _ = self.0.send((lobby, revision));
    }
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<(LobbyId, i64)> {
        self.0.subscribe()
    }
}

pub(crate) struct Connection {
    app: AppState,
    user: AuthenticatedUser,
    membership: String,
    bound: StreamMembership,
    receiver: broadcast::Receiver<(LobbyId, i64)>,
    last_revision: Option<i64>,
    next_read: Instant,
    next_keepalive: Instant,
    ended: bool,
}

impl Connection {
    pub(crate) fn new(
        app: AppState,
        user: AuthenticatedUser,
        membership: String,
        bound: StreamMembership,
        receiver: broadcast::Receiver<(LobbyId, i64)>,
    ) -> Self {
        Self {
            app,
            user,
            membership,
            bound,
            receiver,
            last_revision: None,
            next_read: Instant::now(),
            next_keepalive: Instant::now() + Duration::from_secs(15),
            ended: false,
        }
    }

    fn terminal(&mut self) -> Option<Event> {
        let now = (self.app.auth.clock)();
        let event = if now >= self.user.expires_at {
            Some(Event::default().event("auth_expired").data("{}"))
        } else if now >= self.bound.expires_at.timestamp() {
            Some(
                Event::default()
                    .event("membership_ended")
                    .data(r#"{"reason":"expired"}"#),
            )
        } else {
            None
        };
        if event.is_some() {
            self.ended = true;
        }
        event
    }

    pub(crate) fn stream(self) -> impl Stream<Item = Result<Event, Infallible>> + Send {
        stream::unfold(self, |mut connection| async move {
            if connection.ended {
                return None;
            }
            loop {
                if let Some(event) = connection.terminal() {
                    return Some((Ok(event), connection));
                }
                // The same injected UTC clock drives authorization and timers.
                // Wake at the next wall-clock second to avoid adding a fractional
                // second to a whole-second JWT/lobby deadline.
                let boundary = next_second();
                let mut read = connection.last_revision.is_none();
                if !read {
                    tokio::select! {
                        biased;
                        _ = tokio::time::sleep(boundary) => { read = Instant::now() >= connection.next_read; }
                        _ = tokio::time::sleep_until(connection.next_read) => { read = true; }
                        _ = tokio::time::sleep_until(connection.next_keepalive) => { read = true; }
                        hint = connection.receiver.recv() => {
                            read = match hint {
                                Ok((id, _)) => id == connection.bound.lobby_id,
                                Err(broadcast::error::RecvError::Lagged(_)) => true,
                                Err(broadcast::error::RecvError::Closed) => return None,
                            };
                        }
                    }
                }
                if let Some(event) = connection.terminal() {
                    return Some((Ok(event), connection));
                }
                if !read {
                    continue;
                }
                // Deadline selection can cancel a blocked storage read. It must
                // never delay token expiry until a pool acquisition finishes.
                let state = {
                    let read_state = connection
                        .app
                        .database
                        .stream_membership(connection.user.id, &connection.membership);
                    tokio::pin!(read_state);
                    loop {
                        tokio::select! {
                            biased;
                            _ = tokio::time::sleep(next_second()) => {
                                let now = (connection.app.auth.clock)();
                                if now >= connection.user.expires_at || now >= connection.bound.expires_at.timestamp() {
                                    break None;
                                }
                            }
                            state = &mut read_state => break Some(state),
                        }
                    }
                };
                if let Some(event) = connection.terminal() {
                    return Some((Ok(event), connection));
                }
                let state = match state {
                    Some(Ok(Some(state)))
                        if state.lobby_id == connection.bound.lobby_id
                            && state.closed_at.is_none() =>
                    {
                        state
                    }
                    // US5 adds departure/closure reasons. For now ended seeded
                    // generations and storage failures close without inventing one.
                    _ => return None,
                };
                connection.next_read = Instant::now() + Duration::from_secs(1);
                let initial = connection.last_revision.is_none();
                if initial
                    || connection
                        .last_revision
                        .is_some_and(|last| state.revision > last)
                {
                    connection.last_revision = Some(state.revision);
                    let event = Event::default()
                        .event(if initial {
                            "sync_required"
                        } else {
                            "lobby_changed"
                        })
                        .data(format!(r#"{{"revision":"{}"}}"#, state.revision));
                    return Some((Ok(event), connection));
                }
                if Instant::now() >= connection.next_keepalive {
                    connection.next_keepalive = Instant::now() + Duration::from_secs(15);
                    return Some((Ok(Event::default().comment("keepalive")), connection));
                }
            }
        })
    }
}

fn next_second() -> Duration {
    Duration::from_nanos(1_000_000_000 - u64::from(chrono::Utc::now().timestamp_subsec_nanos()))
}
