//! Game engine orchestration, domain errors and the phase ticker.
pub mod answer;
pub mod images;
pub mod modes;
pub mod rules;

use crate::queue::QueueError;
use crate::{
    app_state::AppState,
    auth::AuthenticatedUser,
    models::game::GameSubmissionResponse,
    models::queue::{EntryId, SubmitRequest},
    youtube::{MetadataError, YouTubeVideo},
};

/// How often the ticker looks for phases and songs whose time ran out.
pub const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Debug)]
pub enum GameError {
    Unauthorized,
    MembershipForbidden,
    HostRequired,
    InvalidRequest,
    TooLarge,
    MediaType,
    ModeUnavailable,
    InvalidRules,
    InvalidSettings,
    GameRunning,
    NotRunning,
    PhaseClosed,
    PhaseChanged,
    SongChanged,
    SubmissionLimit,
    DuplicateSong,
    HostCannotGuess,
    OwnSong,
    GuessLimit,
    InvalidPrompts,
    ImageUnavailable,
    ImageInUse,
    ImageLimit,
    Image(images::ImageError),
    SubmissionUnavailable,
    SelfVote,
    VoteLimit,
    IdempotencyConflict,
    InvalidUrl,
    Upstream,
    Metadata,
    Unavailable,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for GameError {
    fn from(error: sqlx::Error) -> Self {
        QueueError::from(error).into()
    }
}

impl From<QueueError> for GameError {
    fn from(error: QueueError) -> Self {
        match error {
            QueueError::Unauthorized => Self::Unauthorized,
            QueueError::MembershipForbidden => Self::MembershipForbidden,
            QueueError::HostRequired => Self::HostRequired,
            QueueError::Database(error) => Self::Database(error),
            QueueError::InvalidRequest => Self::InvalidRequest,
            _ => Self::Unavailable,
        }
    }
}

/// Resolves a start request to the mode ID it records and the definition it runs.
pub(crate) fn resolve(
    request: &crate::models::game::StartGameRequest,
) -> Result<(String, modes::Definition), GameError> {
    let (mode_id, base) = match (&request.mode_id, &request.custom) {
        (Some(id), None) => (
            id.clone(),
            (modes::preset(id)
                .ok_or(GameError::ModeUnavailable)?
                .definition)(),
        ),
        (None, Some(custom)) => {
            let name = custom.name.trim();
            if !(1..=40).contains(&name.chars().count()) {
                return Err(GameError::InvalidRequest);
            }
            (
                "custom".to_owned(),
                modes::Definition {
                    name: name.to_owned(),
                    rules: custom.rules,
                    settings: modes::default_settings(&custom.rules),
                },
            )
        }
        _ => return Err(GameError::InvalidRequest),
    };
    let definition = modes::customize(base, &request.settings).map_err(|error| match error {
        modes::ModeError::Rules(_) => GameError::InvalidRules,
        modes::ModeError::Setting(_) => GameError::InvalidSettings,
    })?;
    Ok((mode_id, definition))
}

/// Most prompts one game can carry.
pub const MAX_PROMPTS: usize = 50;
/// Longest prompt text, in characters after trimming.
pub const MAX_PROMPT_CHARS: usize = 200;

/// Checks prompt shapes against the prompt rule and trims their text. Whether each
/// image exists in the lobby is checked inside the start transaction.
pub(crate) fn prompts(
    rule: rules::PromptRule,
    input: &[crate::models::game::GamePrompt],
) -> Result<Vec<crate::models::game::GamePrompt>, GameError> {
    use rules::PromptRule;
    if rule == PromptRule::None {
        return if input.is_empty() {
            Ok(Vec::new())
        } else {
            Err(GameError::InvalidPrompts)
        };
    }
    if input.is_empty() || input.len() > MAX_PROMPTS {
        return Err(GameError::InvalidPrompts);
    }
    input
        .iter()
        .map(|prompt| {
            let text = prompt
                .text
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned);
            if text
                .as_ref()
                .is_some_and(|t| t.chars().count() > MAX_PROMPT_CHARS)
            {
                return Err(GameError::InvalidPrompts);
            }
            let image_id = prompt.image_id.clone();
            if image_id
                .as_deref()
                .is_some_and(|id| !crate::models::queue::valid_id(id))
            {
                return Err(GameError::InvalidPrompts);
            }
            let fits = match rule {
                PromptRule::Text => text.is_some() && image_id.is_none(),
                PromptRule::Image => image_id.is_some(),
                PromptRule::None => false,
            };
            if !fits {
                return Err(GameError::InvalidPrompts);
            }
            Ok(crate::models::game::GamePrompt { text, image_id })
        })
        .collect()
}

pub(crate) fn random_id() -> Result<String, GameError> {
    EntryId::random()
        .map(|id| id.as_str().to_owned())
        .map_err(|_| GameError::Unavailable)
}

/// Same validation as the queue: authorize, parse, reuse a matching receipt, then
/// resolve metadata outside the writer and recheck everything inside it.
pub(crate) async fn submit(
    app: &AppState,
    user: &AuthenticatedUser,
    membership: &str,
    request: SubmitRequest,
) -> Result<GameSubmissionResponse, GameError> {
    let video = YouTubeVideo::parse(&request.url).map_err(|_| GameError::InvalidUrl);
    let video_id = video.as_ref().ok().map(|v| v.id.as_str());
    if let Some(response) = app
        .database
        .game_receipt(user, membership, &request.request_id, video_id, || {
            (app.auth.clock)()
        })
        .await?
    {
        return Ok(response);
    }
    let video = video?;
    let metadata = app
        .metadata
        .lookup(&video)
        .await
        .map_err(|error| match error {
            MetadataError::InvalidUrl => GameError::InvalidUrl,
            MetadataError::Service => GameError::Upstream,
            MetadataError::Unavailable => GameError::Metadata,
        })?;
    app.database
        .submit_game(user, membership, &request.request_id, &metadata, || {
            (app.auth.clock)()
        })
        .await
}

/// Advances due phases and songs once per second using the injected clock.
pub fn start_ticker(app: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        loop {
            interval.tick().await;
            match app.database.tick_games((app.auth.clock)()).await {
                Ok(changes) => {
                    for (lobby, revision) in changes {
                        app.lobby_updates.publish(lobby, revision);
                    }
                }
                Err(_) => eprintln!("Game ticker temporarily unavailable"),
            }
        }
    });
}

#[cfg(test)]
mod tests;
