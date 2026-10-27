//! Shared queue submission orchestration and domain errors.
use crate::database::lobbies::LobbyError;
use crate::{
    app_state::AppState,
    auth::AuthenticatedUser,
    models::queue::{EntryId, SubmissionResponse, SubmitRequest},
    youtube::{MetadataError, YouTubeVideo},
};

pub(crate) async fn submit(
    app: &AppState,
    user: &AuthenticatedUser,
    membership: &str,
    request: SubmitRequest,
) -> Result<SubmissionResponse, QueueError> {
    // Authorize before parsing or calling the provider, then reauthorize inside the writer.
    app.database
        .queue_view(user, membership, || (app.auth.clock)())
        .await?;
    let video = YouTubeVideo::parse(&request.url).map_err(|_| QueueError::InvalidUrl)?;
    if let Some(response) = app
        .database
        .queue_receipt(user, membership, &request.request_id, &video.id, || {
            (app.auth.clock)()
        })
        .await?
    {
        return Ok(response);
    }
    let metadata = app
        .metadata
        .lookup(&video)
        .await
        .map_err(|error| match error {
            MetadataError::InvalidUrl => QueueError::InvalidUrl,
            MetadataError::Service => QueueError::Upstream,
            MetadataError::Unavailable => QueueError::Metadata,
        })?;
    app.database
        .submit_queue(
            user,
            membership,
            &request.request_id,
            &metadata,
            || (app.auth.clock)(),
            || EntryId::random().map_err(|_| QueueError::Unavailable),
        )
        .await
}

#[derive(Debug)]
pub enum QueueError {
    Unauthorized,
    MembershipForbidden,
    HostRequired,
    GuestRequired,
    InvalidRequest,
    TooLarge,
    MediaType,
    EntryUnavailable,
    IdempotencyConflict,
    CurrentChanged,
    StaleRevision,
    NotUpcoming,
    InvalidUrl,
    Upstream,
    Metadata,
    Unavailable,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for QueueError {
    fn from(error: sqlx::Error) -> Self {
        match LobbyError::from(error) {
            LobbyError::Unavailable => Self::Unavailable,
            LobbyError::Database(error) => Self::Database(error),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) fn commits()
-> &'static std::sync::Mutex<std::collections::HashMap<(String, i64), std::time::Instant>> {
    static COMMITS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, i64), std::time::Instant>>,
    > = std::sync::OnceLock::new();
    COMMITS.get_or_init(Default::default)
}
