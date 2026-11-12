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
    /// Saved modes: prompts used when a start request sends none. Text only, since
    /// images belong to one lobby.
    pub default_prompts: Vec<GamePrompt>,
}

/// Saves a private custom mode. `settings` are overrides on the rule set's default
/// settings, checked like a start request.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveModeRequest {
    /// 1-40 characters after trimming.
    pub name: String,
    pub rules: Rules,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// Optional default prompts for `text` prompt modes (up to 50).
    #[serde(default)]
    pub prompts: Vec<GamePrompt>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameModesResponse {
    pub modes: Vec<GameModeView>,
}

/// A mode built on the spot: a name and one choice per rule.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomMode {
    /// 1-40 characters after trimming.
    pub name: String,
    pub rules: Rules,
}

/// One round's prompt. `text` rounds need text (1-200 characters); `image` rounds
/// need an uploaded `image_id` and may add text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GamePrompt {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub image_id: Option<String>,
}

/// Exactly one of `mode_id` and `custom`; `settings` overrides the mode's defaults
/// and must satisfy its served schema. `prompts` (up to 50) are used in order, one
/// per round, and are required exactly when the mode's prompt rule is not `none`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartGameRequest {
    pub mode_id: Option<String>,
    pub custom: Option<CustomMode>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub settings: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub prompts: Vec<GamePrompt>,
}

/// The current round's prompt. Fetch `image_id` from `.../game/images/{image_id}`
/// with the bearer token; later prompts are never sent before their round.
#[derive(Debug, Serialize, ToSchema)]
pub struct PromptView {
    /// 1-based position in the prompt list (equal to the round).
    pub number: i64,
    pub text: Option<String>,
    pub image_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameImage {
    pub image_id: String,
    /// `image/jpeg`, or `image/png` for images with transparency.
    pub content_type: String,
    pub width: i64,
    pub height: i64,
    pub bytes: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameImagesResponse {
    pub images: Vec<GameImage>,
    /// Images the lobby may hold at once.
    pub limit: i64,
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
    /// Name That Tune: a hidden clip plays and players race to type its title.
    Guess,
    /// Name That Tune: the answer and the round's guessers are shown.
    Reveal,
    /// The game is over; the scoreboard stays until the next start or dismissal.
    Ended,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Collect => "collect",
            Self::Vote => "vote",
            Self::Play => "play",
            Self::Guess => "guess",
            Self::Reveal => "reveal",
            Self::Ended => "ended",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "collect" => Self::Collect,
            "vote" => Self::Vote,
            "play" => Self::Play,
            "guess" => Self::Guess,
            "reveal" => Self::Reveal,
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
    /// The caller may vote for it: own songs are excluded unless `self_vote`, and byes
    /// in pair ballots take no votes.
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
    /// Votes the caller can cast on this ballot: the smaller of `votes_per_player`
    /// and the songs they may vote for, or one per pair in pair ballots.
    pub votes_allowed: i64,
    pub my_votes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResultEntry {
    pub submission_id: String,
    pub song: ResultSong,
    pub submitted_by_user_id: String,
    /// Filled in on every read from the lobby's current names.
    #[serde(default)]
    pub submitted_by_nickname: String,
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

/// One correct Name That Tune guess, in the order they arrived.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GuessResult {
    pub user_id: String,
    #[serde(default)]
    pub nickname: String,
    /// 1 for the first correct guess.
    pub rank: i64,
    pub points: i64,
}

/// The most recently closed round, revealed to everyone.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RoundResult {
    pub round: i64,
    /// Every submission of the round with its final votes, winners first. In Name
    /// That Tune this is the round's song, with the proposer's points.
    pub entries: Vec<ResultEntry>,
    /// Name That Tune: the title guesses were matched against.
    #[serde(default)]
    pub answer: Option<String>,
    /// Name That Tune: correct guessers, fastest first.
    #[serde(default)]
    pub guesses: Vec<GuessResult>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NowPlaying {
    /// Pass this to `advance` as `play_seq` when the song ends on the host player.
    pub play_seq: i64,
    pub round: i64,
    /// True while a Name That Tune title is secret: `song` and
    /// `submitted_by_user_id` are null for everyone except the host, whose player
    /// needs the video.
    pub hidden: bool,
    pub song: Option<GameSong>,
    pub submitted_by_user_id: Option<String>,
    /// Null exactly when `submitted_by_user_id` is.
    pub submitted_by_nickname: Option<String>,
    pub start_seconds: i64,
    #[schema(value_type = String, format = DateTime)]
    pub started_at: DateTime<Utc>,
    /// Null plays the full song; the host reports its end.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub ends_at: Option<DateTime<Utc>>,
}

/// The caller's standing in the current Name That Tune round (guess and reveal).
#[derive(Debug, Serialize, ToSchema)]
pub struct GuessView {
    /// False for the host (their screen shows the video) and the song's proposer.
    pub eligible: bool,
    /// The caller proposed this round's song: they sit out and score from it.
    pub own_song: bool,
    pub correct: bool,
    pub points: i64,
    pub wrong_guesses: i64,
    pub guesses_left: i64,
    /// Correct guessers so far, without identities until the reveal.
    pub correct_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScoreEntry {
    pub user_id: String,
    /// The player's name in this lobby, kept after they leave.
    pub nickname: String,

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
    /// The current round's prompt in prompt modes while the game runs.
    pub prompt: Option<PromptView>,
    /// The caller's own submissions for this round (Name That Tune: all their proposals).
    pub my_submissions: Vec<OwnSubmission>,
    /// How many songs this round has (Name That Tune: proposals), without identities.
    pub submission_count: i64,
    pub ballot: Option<BallotView>,
    pub last_result: Option<RoundResult>,
    pub now_playing: Option<NowPlaying>,
    /// Present in Name That Tune guess and reveal phases.
    pub guess: Option<GuessView>,
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

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GameGuessRequest {
    /// 1-100 characters after trimming.
    pub text: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GameGuessResponse {
    /// False when the guess was a repeat after an earlier correct one.
    pub changed: bool,
    pub correct: bool,
    /// Points this guess earned (the earlier ones on a repeat).
    pub points: i64,
    pub game: GameSnapshot,
}

/// Exactly one field: `phase_seq` skips that phase; `play_seq` ends that song.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GameAdvanceRequest {
    pub phase_seq: Option<i64>,
    pub play_seq: Option<i64>,
}
