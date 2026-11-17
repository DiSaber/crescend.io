//! Public game representations. Snapshots are per caller: they never carry other
//! players' unrevealed submissions or vote counts before a vote closes.
use crate::game::modes::{Definition, SettingSchema};
use crate::game::rules::Rules;
use crate::models::queue::Revision;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct GameModeView {
    /// Preset key such as `party`, or a saved mode's 32-hex ID.
    pub id: String,
    pub name: String,
    pub description: String,
    pub preset: bool,
    pub rules: Rules,
    pub settings: Vec<SettingSchema>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameModesResponse {
    pub modes: Vec<GameModeView>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartGameRequest {
    pub mode_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GameStatus {
    Running,
    Ended,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Players submit songs for the round.
    Collect,
    /// Players vote on the round's ballot.
    Vote,
    /// The round is decided; its winners play or wait to play.
    Play,
    /// The game is over; the scoreboard stays until the next start or dismissal.
    Ended,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Collect => "collect",
            Self::Vote => "vote",
            Self::Play => "play",
            Self::Ended => "ended",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "collect" => Self::Collect,
            "vote" => Self::Vote,
            "play" => Self::Play,
            "ended" => Self::Ended,
            _ => return None,
        })
    }
}

/// Song details for a submission.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GameSong {
    pub video_id: String,
    pub title: String,
    pub author_name: String,
    pub thumbnail_url: String,
    pub embed_url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OwnSubmission {
    pub id: String,
    pub song: GameSong,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BallotEntry {
    pub submission_id: String,
    pub song: GameSong,
    /// The caller submitted this song.
    pub mine: bool,
    /// The caller may vote for it (own songs are excluded unless `self_vote`).
    pub votable: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BallotGroup {
    pub entries: Vec<BallotEntry>,
}

/// Present only during the vote phase. Submitters stay anonymous; no counts.
#[derive(Debug, Serialize, ToSchema)]
pub struct BallotView {
    pub groups: Vec<BallotGroup>,
    pub votes_per_player: i64,
    pub my_votes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResultEntry {
    pub submission_id: String,
    pub song: ResultSong,
    pub submitted_by_user_id: String,
    pub votes: i64,
    pub winner: bool,
    pub points: i64,
}

/// Stored song snapshot used inside revealed results.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResultSong {
    pub video_id: String,
    pub title: String,
    pub author_name: String,
    pub thumbnail_url: String,
}

/// The most recently closed round, revealed to everyone.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RoundResult {
    pub round: i64,
    /// Every submission of the round with its final votes, winners first.
    pub entries: Vec<ResultEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NowPlaying {
    /// Pass this to `advance` as `play_seq` when the song ends on the host player.
    pub play_seq: i64,
    pub round: i64,
    pub song: GameSong,
    pub submitted_by_user_id: String,
    pub start_seconds: i64,
    #[schema(value_type = String, format = DateTime)]
    pub started_at: DateTime<Utc>,
    /// Null plays the full song; the host reports its end.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScoreEntry {
    pub user_id: String,
    pub points: i64,
    /// False once the account has left the lobby.
    pub active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameView {
    pub id: String,
    pub mode_id: String,
    pub mode: Definition,
    pub status: GameStatus,
    /// Current round, 1-based.
    pub round: i64,
    /// Null when the game runs until stopped.
    pub total_rounds: Option<i64>,
    pub phase: Phase,
    /// Increases on every phase change; pass it to `advance` as `phase_seq`.
    pub phase_seq: i64,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub phase_ends_at: Option<DateTime<Utc>>,
    /// The caller's own submissions for this round.
    pub my_submissions: Vec<OwnSubmission>,
    /// How many songs this round has, without identities.
    pub submission_count: i64,
    pub ballot: Option<BallotView>,
    pub last_result: Option<RoundResult>,
    pub now_playing: Option<NowPlaying>,
    /// Decided songs waiting behind `now_playing`.
    pub up_next_count: i64,
    /// Highest points first; ties by account ID.
    pub scoreboard: Vec<ScoreEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameSnapshot {
    pub membership_id: String,
    pub lobby_id: String,
    /// The lobby revision this snapshot reflects (shared with the queue).
    pub revision: Revision,
    #[schema(value_type = String, format = DateTime)]
    pub server_time: DateTime<Utc>,
    /// Null when no game exists: the plain jukebox.
    pub game: Option<GameView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameMutationResponse {
    pub changed: bool,
    pub game: GameSnapshot,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameSubmissionResponse {
    pub changed: bool,
    pub submission_id: String,
    pub game: GameSnapshot,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GameVoteRequest {
    pub submission_id: String,
}

/// Exactly one field: `phase_seq` skips that phase; `play_seq` ends that song.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GameAdvanceRequest {
    pub phase_seq: Option<i64>,
    pub play_seq: Option<i64>,
}
