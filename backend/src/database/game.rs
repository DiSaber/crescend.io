//! Game engine storage: schema, the phase machine and serialized transactions.
//!
//! Every write runs in one `BEGIN IMMEDIATE` transaction that first catches the
//! game up to the injected clock (`settle`), applies the request, settles again
//! (early phase closes) and bumps the lobby revision once if anything changed.
use super::Database;
use super::queue::{BoundQueue, Permission, authorize, check_deadlines, increment};
use crate::{
    auth::AuthenticatedUser,
    game::{
        GameError,
        answer::{MAX_WRONG_GUESSES, accepts, clean_title},
        images::{self, Processed},
        modes::Definition,
        random_id,
        rules::{BallotRule, ChallengeRule, CollectRule, Entry, outright},
    },
    models::{
        game::*,
        lobby::LobbyId,
        queue::{Revision, timestamp},
        youtube::VideoMetadata,
    },
};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::HashMap;

type Tx<'a> = Transaction<'a, Sqlite>;

/// Upper bound on transitions applied in one settle, so a corrupt state cannot spin.
const MAX_STEPS: usize = 64;

#[derive(Debug, Clone)]
pub(crate) struct Game {
    pub lobby_id: String,
    pub id: String,
    pub mode_id: String,
    pub definition: Definition,
    pub status: GameStatus,
    pub round: i64,
    pub total_rounds: Option<i64>,
    pub phase: Phase,
    pub phase_seq: i64,
    pub phase_ends_at: Option<i64>,
    pub last_result: Option<RoundResult>,
    /// The lobby host: their player shows the video, so they never guess.
    pub owner: i64,
    pub prompts: Vec<GamePrompt>,
}

fn corrupt(what: &str) -> sqlx::Error {
    sqlx::Error::Protocol(format!("invalid stored game {what}"))
}

async fn load(tx: &mut Tx<'_>, lobby: &str) -> Result<Option<Game>, GameError> {
    let Some(row) = sqlx::query("SELECT g.*, l.owner_user_id FROM games g JOIN lobbies l ON l.id = g.lobby_id WHERE g.lobby_id = ?")
        .bind(lobby)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok(None);
    };
    let definition: String = row.try_get("definition")?;
    let last_result: Option<String> = row.try_get("last_result")?;
    let status: String = row.try_get("status")?;
    Ok(Some(Game {
        lobby_id: lobby.into(),
        id: row.try_get("id")?,
        mode_id: row.try_get("mode_id")?,
        definition: serde_json::from_str(&definition).map_err(|_| corrupt("definition"))?,
        status: if status == "running" {
            GameStatus::Running
        } else {
            GameStatus::Ended
        },
        round: row.try_get("round")?,
        total_rounds: row.try_get("total_rounds")?,
        phase: Phase::parse(&row.try_get::<String, _>("phase")?).ok_or_else(|| corrupt("phase"))?,
        phase_seq: row.try_get("phase_seq")?,
        phase_ends_at: row.try_get("phase_ends_at")?,
        last_result: last_result
            .map(|value| serde_json::from_str(&value).map_err(|_| corrupt("result")))
            .transpose()?,
        owner: row.try_get("owner_user_id")?,
        prompts: serde_json::from_str(&row.try_get::<String, _>("prompts")?)
            .map_err(|_| corrupt("prompts"))?,
    }))
}

async fn save(tx: &mut Tx<'_>, game: &Game, now: i64) -> Result<(), GameError> {
    let ended = game.status == GameStatus::Ended;
    sqlx::query("UPDATE games SET status = ?, round = ?, total_rounds = ?, phase = ?, phase_seq = ?, phase_ends_at = ?, last_result = ?,
        ended_at = CASE WHEN ? THEN coalesce(ended_at, ?) ELSE NULL END WHERE id = ?")
        .bind(if ended { "ended" } else { "running" })
        .bind(game.round)
        .bind(game.total_rounds)
        .bind(game.phase.as_str())
        .bind(game.phase_seq)
        .bind(game.phase_ends_at)
        .bind(game.last_result.as_ref().map(|r| serde_json::to_string(r).expect("result JSON")))
        .bind(ended)
        .bind(now)
        .bind(&game.id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn later(now: i64, seconds: i64) -> Result<i64, GameError> {
    let at = now.checked_add(seconds).ok_or(GameError::Unavailable)?;
    timestamp(at).map_err(|_| GameError::Unavailable)?;
    Ok(at)
}

fn enter(game: &mut Game, phase: Phase, ends_at: Option<i64>) {
    game.phase = phase;
    game.phase_ends_at = ends_at;
    game.phase_seq += 1;
}

async fn members(tx: &mut Tx<'_>, lobby: &str) -> Result<Vec<i64>, GameError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM lobby_memberships WHERE lobby_id = ? ORDER BY user_id",
    )
    .bind(lobby)
    .fetch_all(&mut **tx)
    .await?)
}

async fn round_entries(tx: &mut Tx<'_>, game: &Game) -> Result<Vec<Entry>, GameError> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT id, user_id, seq FROM game_submissions WHERE game_id = ? AND round = ? ORDER BY seq",
    )
    .bind(&game.id)
    .bind(game.round)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, user_id, seq)| Entry { id, user_id, seq })
        .collect())
}

/// Ballot groups of the current round, each in submission order.
async fn ballot_groups(tx: &mut Tx<'_>, game: &Game) -> Result<Vec<Vec<Entry>>, GameError> {
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT b.group_index, s.id, s.user_id, s.seq FROM game_ballot b JOIN game_submissions s ON s.id = b.submission_id
         WHERE b.game_id = ? AND b.round = ? ORDER BY b.group_index, s.seq",
    )
    .bind(&game.id)
    .bind(game.round)
    .fetch_all(&mut **tx)
    .await?;
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    let mut last = None;
    for (group, id, user_id, seq) in rows {
        if last != Some(group) {
            groups.push(Vec::new());
            last = Some(group);
        }
        groups
            .last_mut()
            .expect("group pushed")
            .push(Entry { id, user_id, seq });
    }
    Ok(groups)
}

async fn round_votes(tx: &mut Tx<'_>, game: &Game) -> Result<Vec<(String, i64)>, GameError> {
    Ok(sqlx::query_as(
        "SELECT submission_id, user_id FROM game_votes WHERE game_id = ? AND round = ?",
    )
    .bind(&game.id)
    .bind(game.round)
    .fetch_all(&mut **tx)
    .await?)
}

fn may_vote(game: &Game, groups: &[Vec<Entry>], entry: &Entry, user: i64) -> bool {
    let group = groups
        .iter()
        .find(|g| g.contains(entry))
        .map(Vec::as_slice)
        .unwrap_or(std::slice::from_ref(entry));
    game.definition.rules.ballot.imp().eligible(
        group,
        entry,
        user,
        game.definition.settings.self_vote,
    )
}

/// Votes this account still has to cast before it is done with the ballot.
fn votes_needed(game: &Game, groups: &[Vec<Entry>], user: i64) -> i64 {
    game.definition.rules.ballot.imp().needed(
        groups,
        &|entry| may_vote(game, groups, entry, user),
        game.definition.settings.votes_per_player,
    )
}

/// True when every active member has done everything the phase allows. Members away
/// from the lobby for over a minute (no open event stream; `lobbies::presence`) are
/// not waited for; the host always is.
async fn phase_complete(tx: &mut Tx<'_>, game: &Game) -> Result<bool, GameError> {
    let mut members = members(tx, &game.lobby_id).await?;
    let away = crate::lobbies::presence::away(&game.lobby_id);
    members.retain(|user| *user == game.owner || !away.contains(user));
    match game.phase {
        Phase::Collect => {
            let entries = round_entries(tx, game).await?;
            Ok(members.iter().all(|user| {
                entries.iter().filter(|e| e.user_id == *user).count() as i64
                    >= game.definition.settings.songs_per_player
            }))
        }
        Phase::Vote => {
            let groups = ballot_groups(tx, game).await?;
            let votes = round_votes(tx, game).await?;
            Ok(members.iter().all(|user| {
                votes.iter().filter(|(_, voter)| voter == user).count() as i64
                    >= votes_needed(game, &groups, *user)
            }))
        }
        Phase::Guess => {
            let (_, proposer, _) = tune_song(tx, game).await?;
            let correct: Vec<i64> = sqlx::query_scalar(
                "SELECT user_id FROM game_guesses WHERE game_id = ? AND round = ? AND rank IS NOT NULL",
            )
            .bind(&game.id)
            .bind(game.round)
            .fetch_all(&mut **tx)
            .await?;
            Ok(members
                .iter()
                .filter(|user| **user != game.owner && **user != proposer)
                .all(|user| correct.contains(user)))
        }
        Phase::Play | Phase::Reveal | Phase::Ended => Ok(false),
    }
}

fn proposing(game: &Game) -> bool {
    game.definition.rules.collect == CollectRule::Propose
}

/// The song the current Name That Tune round plays: ID, proposer and answer key.
async fn tune_song(tx: &mut Tx<'_>, game: &Game) -> Result<(String, i64, String), GameError> {
    let row: Option<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, user_id, answer FROM game_submissions WHERE game_id = ? AND play_round = ?",
    )
    .bind(&game.id)
    .bind(game.round)
    .fetch_optional(&mut **tx)
    .await?;
    let (id, proposer, answer) = row.ok_or_else(|| corrupt("round song"))?;
    Ok((id, proposer, answer.ok_or_else(|| corrupt("answer"))?))
}

/// Round-robin over proposers (in order of their first proposal), so everyone's
/// first song plays before anyone's second.
fn play_order(entries: &[Entry]) -> Vec<Entry> {
    crate::game::rules::interleave(entries)
}

/// A skipped round's reveal is short: there is nothing to read but the answer.
const SKIP_REVEAL_SECONDS: i64 = 3;

/// Proposals are in: fix the play order and the round count, then start round 1.
async fn begin_guessing(tx: &mut Tx<'_>, game: &mut Game, now: i64) -> Result<(), GameError> {
    let order = play_order(&round_entries(tx, game).await?);
    if order.is_empty() {
        game.last_result = Some(RoundResult {
            round: game.round,
            entries: Vec::new(),
            answer: None,
            guesses: Vec::new(),
            note: Some(ResultNote::NoProposals),
        });
        return end(tx, game).await;
    }
    for (index, entry) in order.iter().enumerate() {
        sqlx::query("UPDATE game_submissions SET play_round = ? WHERE id = ?")
            .bind(index as i64 + 1)
            .bind(&entry.id)
            .execute(&mut **tx)
            .await?;
    }
    let available = order.len() as i64;
    game.total_rounds = Some(match game.definition.settings.rounds {
        0 => available,
        rounds => rounds.min(available),
    });
    game.round = 1;
    start_guess(tx, game, now).await
}

/// Plays the round's song (hidden) and opens guessing for `play_seconds`. When
/// nobody is left who may guess (only the host and the proposer), the round is
/// skipped straight to a short reveal instead of playing to an empty room.
async fn start_guess(tx: &mut Tx<'_>, game: &mut Game, now: i64) -> Result<(), GameError> {
    let (submission, proposer, _) = tune_song(tx, game).await?;
    let members = members(tx, &game.lobby_id).await?;
    if !members.iter().any(|u| *u != game.owner && *u != proposer) {
        tune_result(tx, game, Vec::new(), 0, Some(ResultNote::NobodyToGuess)).await?;
        let seconds = game
            .definition
            .settings
            .reveal_seconds
            .min(SKIP_REVEAL_SECONDS);
        enter(game, Phase::Reveal, Some(later(now, seconds)?));
        return Ok(());
    }
    sqlx::query("INSERT INTO game_plays(game_id, seq, round, submission_id, started_at, finished)
        VALUES (?, (SELECT coalesce(max(seq), 0) + 1 FROM game_plays WHERE game_id = ?), ?, ?, ?, 0)")
        .bind(&game.id).bind(&game.id).bind(game.round).bind(&submission).bind(now)
        .execute(&mut **tx).await?;
    let ends = later(now, game.definition.settings.play_seconds)?;
    enter(game, Phase::Guess, Some(ends));
    Ok(())
}

/// Guessing is over: score the proposer and reveal the answer while the clip plays on.
async fn close_guess(tx: &mut Tx<'_>, game: &mut Game, now: i64) -> Result<(), GameError> {
    let (_, proposer, _) = tune_song(tx, game).await?;
    let guesses: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT user_id, rank, points FROM game_guesses WHERE game_id = ? AND round = ? AND rank IS NOT NULL ORDER BY rank",
    )
    .bind(&game.id)
    .bind(game.round)
    .fetch_all(&mut **tx)
    .await?;
    let points = game
        .definition
        .rules
        .scoring
        .imp()
        .proposer_points(guesses.len() as i64);
    if points > 0 {
        add_points(tx, game, proposer, points).await?;
    }
    tune_result(tx, game, guesses, points, None).await?;
    let ends = later(now, game.definition.settings.reveal_seconds)?;
    enter(game, Phase::Reveal, Some(ends));
    Ok(())
}

/// Reveals the current Name That Tune song as the game's last result.
async fn tune_result(
    tx: &mut Tx<'_>,
    game: &mut Game,
    guesses: Vec<(i64, i64, i64)>,
    points: i64,
    note: Option<ResultNote>,
) -> Result<(), GameError> {
    let (submission, proposer, answer) = tune_song(tx, game).await?;
    let (song, _) = song(tx, &submission).await?;
    game.last_result = Some(RoundResult {
        round: game.round,
        entries: vec![ResultEntry {
            submission_id: submission,
            song: ResultSong {
                video_id: song.video_id,
                title: song.title,
                author_name: song.author_name,
                thumbnail_url: song.thumbnail_url,
            },
            submitted_by_user_id: proposer.to_string(),
            submitted_by_nickname: String::new(),
            votes: 0,
            winner: true,
            points,
            group: 0,
            outcome: ResultOutcome::Won,
        }],
        answer: Some(answer),
        guesses: guesses
            .into_iter()
            .map(|(user, rank, points)| GuessResult {
                user_id: user.to_string(),
                nickname: String::new(),
                rank,
                points,
            })
            .collect(),
        note,
    });
    Ok(())
}

async fn unfinished_plays(
    tx: &mut Tx<'_>,
    game: &Game,
    before_round: Option<i64>,
) -> Result<i64, GameError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM game_plays WHERE game_id = ? AND finished = 0 AND round < ?",
    )
    .bind(&game.id)
    .bind(before_round.unwrap_or(i64::MAX))
    .fetch_one(&mut **tx)
    .await?)
}

fn more_rounds(game: &Game) -> bool {
    game.total_rounds.is_none_or(|total| game.round < total)
}

async fn play_phase_done(tx: &mut Tx<'_>, game: &Game) -> Result<bool, GameError> {
    let blocking = if more_rounds(game) && game.definition.settings.overlap_next_round {
        unfinished_plays(tx, game, Some(game.round)).await?
    } else {
        unfinished_plays(tx, game, None).await?
    };
    Ok(blocking == 0)
}

async fn phase_ready(tx: &mut Tx<'_>, game: &Game, now: i64) -> Result<bool, GameError> {
    if game.status != GameStatus::Running {
        return Ok(false);
    }
    match game.phase {
        Phase::Collect | Phase::Vote | Phase::Guess => {
            Ok(game.phase_ends_at.is_some_and(|at| at <= now) || phase_complete(tx, game).await?)
        }
        Phase::Reveal => Ok(game.phase_ends_at.is_some_and(|at| at <= now)),
        Phase::Play => play_phase_done(tx, game).await,
        Phase::Ended => Ok(false),
    }
}

/// Finishes the head song when its time is up and starts the next one.
async fn advance_plays(tx: &mut Tx<'_>, game: &Game, now: i64) -> Result<bool, GameError> {
    let mut changed = false;
    loop {
        let head: Option<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT seq, started_at, ends_at FROM game_plays WHERE game_id = ? AND finished = 0 ORDER BY seq LIMIT 1",
        )
        .bind(&game.id)
        .fetch_optional(&mut **tx)
        .await?;
        match head {
            None => return Ok(changed),
            Some((seq, None, _)) => {
                let seconds = game.definition.settings.play_seconds;
                let ends = (seconds > 0).then(|| later(now, seconds)).transpose()?;
                sqlx::query(
                    "UPDATE game_plays SET started_at = ?, ends_at = ? WHERE game_id = ? AND seq = ?",
                )
                .bind(now)
                .bind(ends)
                .bind(&game.id)
                .bind(seq)
                .execute(&mut **tx)
                .await?;
                return Ok(true);
            }
            Some((seq, Some(_), Some(ends))) if ends <= now => {
                finish_play(tx, game, seq).await?;
                changed = true;
            }
            Some(_) => return Ok(changed),
        }
    }
}

async fn finish_play(tx: &mut Tx<'_>, game: &Game, seq: i64) -> Result<(), GameError> {
    sqlx::query("UPDATE game_plays SET finished = 1 WHERE game_id = ? AND seq = ?")
        .bind(&game.id)
        .bind(seq)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Leaves the current phase. Callers decide when; this decides what comes next.
async fn transition(tx: &mut Tx<'_>, game: &mut Game, now: i64) -> Result<(), GameError> {
    let settings = game.definition.settings.clone();
    match game.phase {
        Phase::Collect if proposing(game) => begin_guessing(tx, game, now).await?,
        Phase::Guess => close_guess(tx, game, now).await?,
        Phase::Reveal => {
            sqlx::query("UPDATE game_plays SET finished = 1 WHERE game_id = ? AND finished = 0")
                .bind(&game.id)
                .execute(&mut **tx)
                .await?;
            if more_rounds(game) {
                game.round += 1;
                start_guess(tx, game, now).await?;
            } else {
                end(tx, game).await?;
            }
        }
        Phase::Collect => {
            let entries = round_entries(tx, game).await?;
            if entries.len() < 2 {
                // Zero songs skip the round; a single song wins without a vote.
                let groups = game.definition.rules.ballot.imp().groups(&entries);
                close_round(tx, game, &entries, &groups, now).await?;
            } else {
                let groups = game.definition.rules.ballot.imp().groups(&entries);
                for (index, group) in groups.iter().enumerate() {
                    for entry in group {
                        sqlx::query("INSERT INTO game_ballot(submission_id, game_id, round, group_index) VALUES (?, ?, ?, ?)")
                            .bind(&entry.id).bind(&game.id).bind(game.round).bind(index as i64)
                            .execute(&mut **tx).await?;
                    }
                }
                enter(game, Phase::Vote, Some(later(now, settings.vote_seconds)?));
            }
        }
        Phase::Vote => {
            let entries = round_entries(tx, game).await?;
            let groups = ballot_groups(tx, game).await?;
            close_round(tx, game, &entries, &groups, now).await?;
        }
        Phase::Play => {
            if more_rounds(game) {
                game.round += 1;
                enter(
                    game,
                    Phase::Collect,
                    Some(later(now, settings.collect_seconds)?),
                );
            } else {
                end(tx, game).await?;
            }
        }
        Phase::Ended => {}
    }
    Ok(())
}

/// Decides winners, scores the round, reveals it and queues the winners to play.
async fn close_round(
    tx: &mut Tx<'_>,
    game: &mut Game,
    entries: &[Entry],
    groups: &[Vec<Entry>],
    _now: i64,
) -> Result<(), GameError> {
    let rules = game.definition.rules;
    let mut votes: HashMap<String, i64> = HashMap::new();
    for (submission, _) in round_votes(tx, game).await? {
        *votes.entry(submission).or_insert(0) += 1;
    }
    let winners = rules.winner.imp().winners(groups, &votes);
    // Points per song; the scoreboard adds up each player's songs.
    let points = rules.scoring.imp().round_points(groups, &winners, &votes);
    let mut per_player: HashMap<i64, i64> = HashMap::new();
    for entry in groups.iter().flatten() {
        if let Some(amount) = points.get(&entry.id) {
            *per_player.entry(entry.user_id).or_insert(0) += amount;
        }
    }
    for (user, amount) in &per_player {
        add_points(tx, game, *user, *amount).await?;
    }
    let pairs = rules.ballot == BallotRule::Pairs;
    let mut seq: i64 =
        sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM game_plays WHERE game_id = ?")
            .bind(&game.id)
            .fetch_one(&mut **tx)
            .await?;
    for winner in &winners {
        seq += 1;
        sqlx::query("INSERT INTO game_plays(game_id, seq, round, submission_id, finished) VALUES (?, ?, ?, ?, 0)")
            .bind(&game.id).bind(seq).bind(game.round).bind(&winner.id)
            .execute(&mut **tx).await?;
    }
    let mut result = Vec::with_capacity(entries.len());
    for entry in entries {
        let song: (String, String, String, String) = sqlx::query_as(
            "SELECT video_id, title, author_name, thumbnail_url FROM game_submissions WHERE id = ?",
        )
        .bind(&entry.id)
        .fetch_one(&mut **tx)
        .await?;
        let won = winners.iter().any(|w| w.id == entry.id);
        let group_index = groups.iter().position(|g| g.contains(entry));
        let group = group_index.map(|i| groups[i].as_slice()).unwrap_or(&[]);
        let outcome = if pairs && group.len() < 2 {
            ResultOutcome::Bye
        } else if pairs && !outright(group, &votes) {
            ResultOutcome::Tie
        } else if won {
            ResultOutcome::Won
        } else {
            ResultOutcome::Lost
        };
        result.push(ResultEntry {
            submission_id: entry.id.clone(),
            song: ResultSong {
                video_id: song.0,
                title: song.1,
                author_name: song.2,
                thumbnail_url: song.3,
            },
            submitted_by_user_id: entry.user_id.to_string(),
            submitted_by_nickname: String::new(),
            votes: votes.get(&entry.id).copied().unwrap_or(0),
            winner: won,
            points: points.get(&entry.id).copied().unwrap_or(0),
            group: group_index.unwrap_or(0) as i64,
            outcome,
        });
    }
    // Pairs stay together (winner first in each); one ballot lists winners first.
    result.sort_by_key(|e| (e.group, !e.winner));
    game.last_result = Some(RoundResult {
        round: game.round,
        entries: result,
        answer: None,
        guesses: Vec::new(),
        note: None,
    });
    enter(game, Phase::Play, None);
    Ok(())
}

async fn add_points(tx: &mut Tx<'_>, game: &Game, user: i64, amount: i64) -> Result<(), GameError> {
    sqlx::query(
        "INSERT INTO game_scores(game_id, user_id, points) VALUES (?, ?, ?)
        ON CONFLICT(game_id, user_id) DO UPDATE SET points = points + excluded.points",
    )
    .bind(&game.id)
    .bind(user)
    .bind(amount)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn end(tx: &mut Tx<'_>, game: &mut Game) -> Result<(), GameError> {
    sqlx::query("UPDATE game_plays SET finished = 1 WHERE game_id = ? AND finished = 0")
        .bind(&game.id)
        .execute(&mut **tx)
        .await?;
    game.status = GameStatus::Ended;
    enter(game, Phase::Ended, None);
    Ok(())
}

/// Applies every transition that is due at `now`. Returns whether state changed.
async fn settle(tx: &mut Tx<'_>, game: &mut Game, now: i64) -> Result<bool, GameError> {
    let mut changed = false;
    for _ in 0..MAX_STEPS {
        if game.status != GameStatus::Running {
            break;
        }
        let mut step = advance_plays(tx, game, now).await?;
        if phase_ready(tx, game, now).await? {
            transition(tx, game, now).await?;
            step = true;
        }
        if !step {
            break;
        }
        changed = true;
    }
    if changed {
        save(tx, game, now).await?;
    }
    Ok(changed)
}

/// A member left the lobby. Their departure can complete the open phase (everyone
/// still here has picked, voted or guessed), so settle now rather than at the timer.
/// The caller's commit bumps the revision.
pub(super) async fn settle_after_leave(
    tx: &mut Tx<'_>,
    lobby: &str,
    now: i64,
) -> Result<bool, GameError> {
    let Some(mut game) = load(tx, lobby).await? else {
        return Ok(false);
    };
    settle(tx, &mut game, now).await
}

async fn song(tx: &mut Tx<'_>, submission: &str) -> Result<(GameSong, i64), GameError> {
    let row = sqlx::query("SELECT * FROM game_submissions WHERE id = ?")
        .bind(submission)
        .fetch_one(&mut **tx)
        .await?;
    Ok((
        GameSong {
            video_id: row.try_get("video_id")?,
            title: row.try_get("title")?,
            author_name: row.try_get("author_name")?,
            thumbnail_url: row.try_get("thumbnail_url")?,
            embed_url: row.try_get("embed_url")?,
        },
        row.try_get("user_id")?,
    ))
}

fn utc(seconds: i64) -> Result<chrono::DateTime<chrono::Utc>, GameError> {
    timestamp(seconds).map_err(|_| GameError::Unavailable)
}

/// No account has ID 0: the board view reads the game as a member who owns
/// nothing, never the host, so it gets exactly what every guesser may see.
pub(crate) const BOARD_VIEWER: i64 = 0;

async fn view(tx: &mut Tx<'_>, game: &Game, user: i64) -> Result<GameView, GameError> {
    let names = super::nicknames::lobby_names(&mut **tx, &game.lobby_id).await?;
    let name = |id: i64| super::nicknames::name_of(&names, id);
    let named = |text: &str| text.parse::<i64>().map(name).unwrap_or_default();
    // Name That Tune proposals all belong to the single collect round.
    let submission_round = if proposing(game) { 1 } else { game.round };
    let mine: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM game_submissions WHERE game_id = ? AND round = ? AND user_id = ? ORDER BY seq",
    )
    .bind(&game.id)
    .bind(submission_round)
    .bind(user)
    .fetch_all(&mut **tx)
    .await?;
    let mut my_submissions = Vec::with_capacity(mine.len());
    for id in mine {
        my_submissions.push(OwnSubmission {
            song: song(tx, &id).await?.0,
            id,
        });
    }
    let submission_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM game_submissions WHERE game_id = ? AND round = ?")
            .bind(&game.id)
            .bind(submission_round)
            .fetch_one(&mut **tx)
            .await?;
    let ballot = if game.phase == Phase::Vote {
        let groups = ballot_groups(tx, game).await?;
        let my_votes: Vec<String> = round_votes(tx, game)
            .await?
            .into_iter()
            .filter(|(_, voter)| *voter == user)
            .map(|(submission, _)| submission)
            .collect();
        let mut out = Vec::with_capacity(groups.len());
        for group in groups.clone() {
            let mut entries = Vec::with_capacity(group.len());
            for entry in group {
                entries.push(BallotEntry {
                    song: song(tx, &entry.id).await?.0,
                    mine: entry.user_id == user,
                    votable: may_vote(game, &groups, &entry, user)
                        && game.definition.rules.ballot.imp().allows(
                            &groups,
                            &[],
                            &entry.id,
                            game.definition.settings.votes_per_player,
                        ),
                    submission_id: entry.id,
                });
            }
            out.push(BallotGroup { entries });
        }
        Some(BallotView {
            groups: out,
            votes_allowed: votes_needed(game, &groups, user),
            my_votes,
        })
    } else {
        None
    };
    let plays: Vec<(i64, i64, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT seq, round, submission_id, started_at, ends_at FROM game_plays WHERE game_id = ? AND finished = 0 ORDER BY seq",
    )
    .bind(&game.id)
    .fetch_all(&mut **tx)
    .await?;
    let mut now_playing = None;
    let mut up_next_count = plays.len() as i64;
    if let Some((seq, round, submission, Some(started), ends)) = plays.first().cloned() {
        up_next_count -= 1;
        let (song, picker) = song(tx, &submission).await?;
        // The title stays secret while players guess; the host's player needs the video.
        let hidden = game.definition.rules.challenge == ChallengeRule::GuessTitle
            && game.phase == Phase::Guess
            && user != game.owner;
        now_playing = Some(NowPlaying {
            play_seq: seq,
            round,
            hidden,
            song: (!hidden).then_some(song),
            submitted_by_user_id: (!hidden).then(|| picker.to_string()),
            submitted_by_nickname: (!hidden).then(|| name(picker)),
            start_seconds: game.definition.settings.clip_start_seconds,
            started_at: utc(started)?,
            ends_at: ends.map(utc).transpose()?,
            prompt: prompt_for(game, round),
        });
    }
    let guess = if matches!(game.phase, Phase::Guess | Phase::Reveal) {
        let (_, proposer, _) = tune_song(tx, game).await?;
        let mine: Option<(i64, Option<i64>, i64)> = sqlx::query_as(
            "SELECT wrong, rank, points FROM game_guesses WHERE game_id = ? AND round = ? AND user_id = ?",
        )
        .bind(&game.id)
        .bind(game.round)
        .bind(user)
        .fetch_optional(&mut **tx)
        .await?;
        let correct_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM game_guesses WHERE game_id = ? AND round = ? AND rank IS NOT NULL",
        )
        .bind(&game.id)
        .bind(game.round)
        .fetch_one(&mut **tx)
        .await?;
        let (wrong, rank, points) = mine.unwrap_or((0, None, 0));
        let eligible = user != game.owner && user != proposer;
        let open = eligible && rank.is_none() && game.phase == Phase::Guess;
        Some(GuessView {
            eligible,
            own_song: user == proposer,
            correct: rank.is_some(),
            points,
            wrong_guesses: wrong,
            guesses_left: if open {
                (MAX_WRONG_GUESSES - wrong).max(0)
            } else {
                0
            },
            correct_count,
        })
    } else {
        None
    };
    let active = members(tx, &game.lobby_id).await?;
    let mut scores: HashMap<i64, i64> = sqlx::query_as::<_, (i64, i64)>(
        "SELECT user_id, points FROM game_scores WHERE game_id = ?",
    )
    .bind(&game.id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect();
    for member in &active {
        scores.entry(*member).or_insert(0);
    }
    // Active members always show; departed ones only when they scored.
    let mut scoreboard: Vec<(i64, i64)> = scores
        .into_iter()
        .filter(|(user_id, points)| *points > 0 || active.contains(user_id))
        .collect();
    scoreboard.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ok(GameView {
        id: game.id.clone(),
        mode_id: game.mode_id.clone(),
        mode: game.definition.clone(),
        status: game.status,
        round: game.round,
        total_rounds: game.total_rounds,
        phase: game.phase,
        phase_seq: game.phase_seq,
        phase_ends_at: game.phase_ends_at.map(utc).transpose()?,
        prompt: current_prompt(game),
        my_submissions,
        submission_count,
        ballot,
        last_result: game.last_result.clone().map(|mut result| {
            for entry in &mut result.entries {
                entry.submitted_by_nickname = named(&entry.submitted_by_user_id);
            }
            for guess in &mut result.guesses {
                guess.nickname = named(&guess.user_id);
            }
            result
        }),
        now_playing,
        guess,
        up_next_count,
        scoreboard: scoreboard
            .into_iter()
            .map(|(user_id, points)| ScoreEntry {
                user_id: user_id.to_string(),
                nickname: name(user_id),

                points,
                active: active.contains(&user_id),
            })
            .collect(),
    })
}

/// Only the round's own prompt is ever sent; later prompts stay with the host.
fn current_prompt(game: &Game) -> Option<PromptView> {
    prompt_for(game, game.round)
}

/// The prompt of `round` while the game runs, for a round the game has reached (the
/// current one, or an earlier one whose song is still playing under overlap).
fn prompt_for(game: &Game, round: i64) -> Option<PromptView> {
    if game.status != GameStatus::Running || round < 1 || round > game.round {
        return None;
    }
    let index = usize::try_from(round - 1).ok()?;
    game.prompts.get(index).map(|prompt| PromptView {
        number: round,
        text: prompt.text.clone(),
        image_id: prompt.image_id.clone(),
    })
}

/// Images players may already see: those of prompts up to the current round.
fn revealed_images(game: &Game) -> impl Iterator<Item = &str> {
    let shown = usize::try_from(game.round).unwrap_or(0);
    game.prompts
        .iter()
        .take(shown)
        .filter_map(|p| p.image_id.as_deref())
}

async fn snapshot(
    tx: &mut Tx<'_>,
    bound: &BoundQueue,
    user: i64,
    membership: &str,
    now: i64,
) -> Result<GameSnapshot, GameError> {
    let game = match load(tx, &bound.lobby_id).await? {
        Some(game) => Some(view(tx, &game, user).await?),
        None => None,
    };
    Ok(GameSnapshot {
        membership_id: membership.into(),
        lobby_id: bound.lobby_id.clone(),
        revision: bound.revision,
        server_time: utc(now)?,
        game,
    })
}

/// Loads the lobby's game and catches it up to `now`.
async fn current(
    tx: &mut Tx<'_>,
    bound: &BoundQueue,
    now: i64,
) -> Result<(Option<Game>, bool), GameError> {
    let Some(mut game) = load(tx, &bound.lobby_id).await? else {
        return Ok((None, false));
    };
    let changed = settle(tx, &mut game, now).await?;
    Ok((Some(game), changed))
}

fn running(game: Option<Game>) -> Result<Game, GameError> {
    game.filter(|g| g.status == GameStatus::Running)
        .ok_or(GameError::NotRunning)
}

/// Commits a write: bumps the revision once when anything changed.
async fn finish(
    mut tx: Tx<'_>,
    mut bound: BoundQueue,
    user: &AuthenticatedUser,
    membership: &str,
    changed: bool,
    now: impl Fn() -> i64,
) -> Result<GameSnapshot, GameError> {
    if changed {
        increment(&mut tx, &mut bound).await?;
    }
    let view = snapshot(&mut tx, &bound, user.id, membership, now()).await?;
    check_deadlines(&bound, user, now())?;
    tx.commit().await?;
    #[cfg(test)]
    if changed {
        super::queue::record_commit(&bound.lobby_id, bound.revision.value());
    }
    Ok(view)
}

impl Database {
    /// `board`: the shared TV view, which gets the guest-visible snapshot even on
    /// the host's account (no answer, no video while guessing, no own picks).
    pub(crate) async fn game_view(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        board: bool,
        clock: impl Fn() -> i64,
    ) -> Result<GameSnapshot, GameError> {
        let mut tx = self.db_pool.begin().await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let viewer = if board { BOARD_VIEWER } else { user.id };
        let view = snapshot(&mut tx, &bound, viewer, membership, clock()).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        Ok(view)
    }

    pub(crate) async fn start_game(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        mode_id: &str,
        definition: &Definition,
        prompts: &[GamePrompt],
        clock: impl Fn() -> i64,
    ) -> Result<GameSnapshot, GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        let (existing, _) = current(&mut tx, &bound, clock()).await?;
        if existing.is_some_and(|g| g.status == GameStatus::Running) {
            return Err(GameError::GameRunning);
        }
        // With two players each sits out the pair holding their own song, so
        // nobody could vote in a two-song duel.
        if definition.rules.ballot == BallotRule::Pairs
            && members(&mut tx, &bound.lobby_id).await?.len() < 3
        {
            return Err(GameError::NotEnoughPlayers(
                "Duel needs at least 3 players: with 2, nobody can vote in the pair.",
            ));
        }
        for image in prompts.iter().filter_map(|p| p.image_id.as_deref()) {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM game_images WHERE id = ? AND lobby_id = ?)",
            )
            .bind(image)
            .bind(&bound.lobby_id)
            .fetch_one(&mut *tx)
            .await?;
            if !exists {
                return Err(GameError::ImageUnavailable);
            }
        }
        sqlx::query("DELETE FROM games WHERE lobby_id = ?")
            .bind(&bound.lobby_id)
            .execute(&mut *tx)
            .await?;
        let now = clock();
        let settings = &definition.settings;
        // Prompt modes play one round per prompt, capped by `rounds` when it is set.
        let total = match (prompts.len() as i64, settings.rounds) {
            (0, 0) => None,
            (0, rounds) => Some(rounds),
            (available, 0) => Some(available),
            (available, rounds) => Some(rounds.min(available)),
        };
        sqlx::query("INSERT INTO games(lobby_id, id, mode_id, definition, status, round, total_rounds, phase, phase_seq, phase_ends_at, started_by_user_id, started_at, prompts)
            VALUES (?, ?, ?, ?, 'running', 1, ?, 'collect', 1, ?, ?, ?, ?)")
            .bind(&bound.lobby_id)
            .bind(random_id()?)
            .bind(mode_id)
            .bind(serde_json::to_string(&definition).expect("definition JSON"))
            .bind(total)
            .bind(later(now, settings.collect_seconds)?)
            .bind(user.id)
            .bind(now)
            .bind(serde_json::to_string(prompts).expect("prompts JSON"))
            .execute(&mut *tx)
            .await?;
        super::game_modes::log_mode(
            &mut tx,
            user.id,
            super::game_modes::LogEvent::Started,
            mode_id,
            definition,
            now,
        )
        .await?;
        finish(tx, bound, user, membership, true, clock).await
    }

    pub(crate) async fn stop_game(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        clock: impl Fn() -> i64,
    ) -> Result<GameMutationResponse, GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        let now = clock();
        let (game, _) = current(&mut tx, &bound, now).await?;
        let changed = match game {
            None => false,
            Some(mut game) if game.status == GameStatus::Running => {
                // Ending mid-guess reveals the song on the final screen; the
                // proposer still scores for whoever named it.
                if game.phase == Phase::Guess {
                    close_guess(&mut tx, &mut game, now).await?;
                    if let Some(result) = game.last_result.as_mut() {
                        result.note = Some(ResultNote::EndedEarly);
                    }
                }
                end(&mut tx, &mut game).await?;
                save(&mut tx, &game, now).await?;
                true
            }
            Some(_) => {
                sqlx::query("DELETE FROM games WHERE lobby_id = ?")
                    .bind(&bound.lobby_id)
                    .execute(&mut *tx)
                    .await?;
                true
            }
        };
        let game = finish(tx, bound, user, membership, changed, clock).await?;
        Ok(GameMutationResponse { changed, game })
    }

    pub(crate) async fn advance_game(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        request: &GameAdvanceRequest,
        clock: impl Fn() -> i64,
    ) -> Result<GameMutationResponse, GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        let now = clock();
        let (game, _) = current(&mut tx, &bound, now).await?;
        let mut game = running(game)?;
        match (request.phase_seq, request.play_seq) {
            (Some(seq), None) => {
                if seq != game.phase_seq {
                    return Err(GameError::PhaseChanged);
                }
                if game.phase == Phase::Play {
                    // Skipping the waiting phase skips the song that holds it up.
                    let head: Option<i64> = sqlx::query_scalar("SELECT seq FROM game_plays WHERE game_id = ? AND finished = 0 ORDER BY seq LIMIT 1")
                        .bind(&game.id).fetch_optional(&mut *tx).await?;
                    if let Some(head) = head {
                        finish_play(&mut tx, &game, head).await?;
                    }
                }
                if game.phase != Phase::Play || play_phase_done(&mut tx, &game).await? {
                    transition(&mut tx, &mut game, now).await?;
                }
            }
            (None, Some(seq)) => {
                let head: Option<i64> = sqlx::query_scalar("SELECT seq FROM game_plays WHERE game_id = ? AND finished = 0 AND started_at IS NOT NULL ORDER BY seq LIMIT 1")
                    .bind(&game.id).fetch_optional(&mut *tx).await?;
                if head != Some(seq) {
                    return Err(GameError::SongChanged);
                }
                finish_play(&mut tx, &game, seq).await?;
            }
            _ => return Err(GameError::InvalidRequest),
        }
        save(&mut tx, &game, now).await?;
        settle(&mut tx, &mut game, now).await?;
        let game = finish(tx, bound, user, membership, true, clock).await?;
        Ok(GameMutationResponse {
            changed: true,
            game,
        })
    }

    /// Read-only pre-check before the metadata lookup: authorization, phase and receipts.
    pub(crate) async fn game_receipt(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        request: &str,
        video: Option<&str>,
        clock: impl Fn() -> i64,
    ) -> Result<Option<GameSubmissionResponse>, GameError> {
        let mut tx = self.db_pool.begin().await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let game = running(load(&mut tx, &bound.lobby_id).await?)?;
        let prior = match video {
            Some(video) => receipt(&mut tx, &game, user.id, request, video).await?,
            None => None,
        };
        let response = match prior {
            Some(id) => Some(GameSubmissionResponse {
                changed: false,
                submission_id: id,
                game: snapshot(&mut tx, &bound, user.id, membership, clock()).await?,
            }),
            None => {
                if game.phase != Phase::Collect {
                    return Err(GameError::PhaseClosed);
                }
                None
            }
        };
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        Ok(response)
    }

    pub(crate) async fn submit_game(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        request: &str,
        video: &VideoMetadata,
        clock: impl Fn() -> i64,
    ) -> Result<GameSubmissionResponse, GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let now = clock();
        let (game, mut changed) = current(&mut tx, &bound, now).await?;
        let mut game = running(game)?;
        let mut inserted = false;
        let id = if let Some(id) =
            receipt(&mut tx, &game, user.id, request, &video.video_id).await?
        {
            id
        } else {
            if game.phase != Phase::Collect {
                return Err(GameError::PhaseClosed);
            }
            let used: i64 = sqlx::query_scalar("SELECT count(*) FROM game_submissions WHERE game_id = ? AND round = ? AND user_id = ?")
                .bind(&game.id).bind(game.round).bind(user.id).fetch_one(&mut *tx).await?;
            if used >= game.definition.settings.songs_per_player {
                return Err(GameError::SubmissionLimit);
            }
            if proposing(&game) {
                // A song plays once per game, so a second proposal of it is refused.
                let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM game_submissions WHERE game_id = ? AND video_id = ?)")
                    .bind(&game.id).bind(&video.video_id).fetch_one(&mut *tx).await?;
                if taken {
                    return Err(GameError::DuplicateSong);
                }
            } else {
                // One copy of a video per round, so the ballot never lists it twice.
                let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM game_submissions WHERE game_id = ? AND round = ? AND video_id = ?)")
                    .bind(&game.id).bind(game.round).bind(&video.video_id).fetch_one(&mut *tx).await?;
                if taken {
                    return Err(GameError::SongAlreadyPicked);
                }
            }
            let id = random_id()?;
            let answer = answer_key(&mut tx, video).await?;
            sqlx::query("INSERT INTO game_submissions(id, game_id, round, user_id, request_id, seq, video_id, title, author_name, thumbnail_url, embed_url, submitted_at, answer)
                VALUES (?, ?, ?, ?, ?, (SELECT coalesce(max(seq), 0) + 1 FROM game_submissions WHERE game_id = ?), ?, ?, ?, ?, ?, ?, ?)")
                .bind(&id).bind(&game.id).bind(game.round).bind(user.id).bind(request).bind(&game.id)
                .bind(&video.video_id).bind(&video.title).bind(&video.author_name)
                .bind(&video.thumbnail_url).bind(&video.embed_url).bind(now).bind(&answer)
                .execute(&mut *tx).await?;
            settle(&mut tx, &mut game, now).await?;
            inserted = true;
            id
        };
        changed |= inserted;
        let game = finish(tx, bound, user, membership, changed, clock).await?;
        Ok(GameSubmissionResponse {
            changed: inserted,
            submission_id: id,
            game,
        })
    }

    pub(crate) async fn guess_game(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        text: &str,
        clock: impl Fn() -> i64,
    ) -> Result<GameGuessResponse, GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let now = clock();
        let (game, mut changed) = current(&mut tx, &bound, now).await?;
        let mut game = running(game)?;
        if game.phase != Phase::Guess {
            return Err(GameError::PhaseClosed);
        }
        if user.id == game.owner {
            return Err(GameError::HostCannotGuess);
        }
        let (_, proposer, answer) = tune_song(&mut tx, &game).await?;
        if user.id == proposer {
            return Err(GameError::OwnSong);
        }
        let prior: Option<(i64, Option<i64>, i64)> = sqlx::query_as(
            "SELECT wrong, rank, points FROM game_guesses WHERE game_id = ? AND round = ? AND user_id = ?",
        )
        .bind(&game.id)
        .bind(game.round)
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?;
        let (wrong, rank, earned) = prior.unwrap_or((0, None, 0));
        // The limit counts wrong guesses only: a right answer is accepted even after
        // the last wrong one, and a wrong one past the limit is refused uncounted.
        let right = rank.is_none() && accepts(text, &answer);
        let (guessed, correct, points) = if rank.is_some() {
            (false, true, earned)
        } else if !right && wrong >= MAX_WRONG_GUESSES {
            return Err(GameError::GuessLimit);
        } else if right {
            let rank: i64 = sqlx::query_scalar(
                "SELECT count(*) + 1 FROM game_guesses WHERE game_id = ? AND round = ? AND rank IS NOT NULL",
            )
            .bind(&game.id)
            .bind(game.round)
            .fetch_one(&mut *tx)
            .await?;
            let remaining = game.phase_ends_at.unwrap_or(now) - now;
            let points = game.definition.rules.scoring.imp().guess_points(
                rank,
                remaining,
                game.definition.settings.play_seconds,
            );
            sqlx::query("INSERT INTO game_guesses(game_id, round, user_id, wrong, rank, points, correct_at) VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(game_id, round, user_id) DO UPDATE SET rank = excluded.rank, points = excluded.points, correct_at = excluded.correct_at")
                .bind(&game.id).bind(game.round).bind(user.id).bind(wrong).bind(rank).bind(points).bind(now)
                .execute(&mut *tx).await?;
            add_points(&mut tx, &game, user.id, points).await?;
            (true, true, points)
        } else {
            sqlx::query(
                "INSERT INTO game_guesses(game_id, round, user_id, wrong) VALUES (?, ?, ?, 1)
                ON CONFLICT(game_id, round, user_id) DO UPDATE SET wrong = wrong + 1",
            )
            .bind(&game.id)
            .bind(game.round)
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
            (true, false, 0)
        };
        if guessed {
            settle(&mut tx, &mut game, now).await?;
            changed = true;
        }
        let game = finish(tx, bound, user, membership, changed, clock).await?;
        Ok(GameGuessResponse {
            changed: guessed,
            correct,
            points,
            game,
        })
    }

    pub(crate) async fn vote_game(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        submission: &str,
        clock: impl Fn() -> i64,
    ) -> Result<GameMutationResponse, GameError> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let now = clock();
        let (game, mut changed) = current(&mut tx, &bound, now).await?;
        let mut game = running(game)?;
        if game.phase != Phase::Vote {
            return Err(GameError::PhaseClosed);
        }
        let groups = ballot_groups(&mut tx, &game).await?;
        let entry = groups
            .iter()
            .flatten()
            .find(|e| e.id == submission)
            .cloned()
            .ok_or(GameError::SubmissionUnavailable)?;
        if !may_vote(&game, &groups, &entry, user.id) {
            return Err(if entry.user_id == user.id {
                GameError::SelfVote
            } else {
                GameError::OwnPair
            });
        }
        let votes = round_votes(&mut tx, &game).await?;
        let mine: Vec<String> = votes
            .into_iter()
            .filter(|(_, voter)| *voter == user.id)
            .map(|(s, _)| s)
            .collect();
        let voted = if mine.iter().any(|s| s == submission) {
            false
        } else {
            if !game.definition.rules.ballot.imp().allows(
                &groups,
                &mine,
                submission,
                game.definition.settings.votes_per_player,
            ) {
                return Err(GameError::VoteLimit);
            }
            sqlx::query("INSERT INTO game_votes(submission_id, user_id, game_id, round) VALUES (?, ?, ?, ?)")
                .bind(submission).bind(user.id).bind(&game.id).bind(game.round)
                .execute(&mut *tx).await?;
            settle(&mut tx, &mut game, now).await?;
            true
        };
        changed |= voted;
        let game = finish(tx, bound, user, membership, changed, clock).await?;
        Ok(GameMutationResponse {
            changed: voted,
            game,
        })
    }

    /// Advances every running game whose phase or song is due. Each lobby commits
    /// in its own transaction; returned pairs are published by the caller.
    pub(crate) async fn tick_games(&self, now: i64) -> Result<Vec<(LobbyId, i64)>, GameError> {
        let at = utc(now)?;
        let due: Vec<LobbyId> = sqlx::query_scalar(
            "SELECT g.lobby_id FROM games g JOIN lobbies l ON l.id = g.lobby_id
             WHERE g.status = 'running' AND l.closed_at IS NULL AND l.expires_at > ?
             AND (g.phase = 'play' OR g.phase_ends_at <= ? OR EXISTS(SELECT 1 FROM game_plays p
                 WHERE p.game_id = g.id AND p.finished = 0 AND (p.started_at IS NULL OR p.ends_at <= ?)))",
        )
        .bind(at)
        .bind(now)
        .bind(now)
        .fetch_all(&self.db_pool)
        .await?;
        let mut changes = Vec::new();
        for lobby in due {
            let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
            let Some(mut game) = load(&mut tx, lobby.as_str()).await? else {
                continue;
            };
            if !settle(&mut tx, &mut game, now).await? {
                continue;
            }
            let revision: i64 = sqlx::query_scalar("SELECT revision FROM lobbies WHERE id = ?")
                .bind(lobby.as_str())
                .fetch_one(&mut *tx)
                .await?;
            let revision = Revision::new(revision)
                .and_then(Revision::next)
                .map_err(|_| GameError::Unavailable)?;
            sqlx::query("UPDATE lobbies SET revision = ? WHERE id = ?")
                .bind(revision.value())
                .bind(lobby.as_str())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            #[cfg(test)]
            super::queue::record_commit(lobby.as_str(), revision.value());
            changes.push((lobby, revision.value()));
        }
        Ok(changes)
    }

    pub(super) async fn create_game_tables(&self) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        for statement in [
            "CREATE TABLE IF NOT EXISTS games (
                lobby_id TEXT PRIMARY KEY NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                id TEXT NOT NULL UNIQUE CHECK(length(id) = 32 AND id NOT GLOB '*[^0-9a-f]*'),
                mode_id TEXT NOT NULL CHECK(length(mode_id) BETWEEN 1 AND 64),
                definition TEXT NOT NULL CHECK(json_valid(definition)),
                status TEXT NOT NULL CHECK(status IN ('running', 'ended')),
                round INTEGER NOT NULL CHECK(round >= 1),
                total_rounds INTEGER CHECK(total_rounds IS NULL OR total_rounds >= 1),
                phase TEXT NOT NULL CHECK(phase IN ('collect', 'vote', 'play', 'guess', 'reveal', 'ended')),
                phase_seq INTEGER NOT NULL CHECK(phase_seq >= 1),
                phase_ends_at INTEGER,
                started_by_user_id INTEGER NOT NULL REFERENCES users(id),
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                last_result TEXT CHECK(last_result IS NULL OR json_valid(last_result)),
                prompts TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(prompts)),
                CHECK((status = 'ended') = (phase = 'ended')),
                CHECK((status = 'ended') = (ended_at IS NOT NULL))
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS game_submissions (
                id TEXT PRIMARY KEY NOT NULL CHECK(length(id) = 32 AND id NOT GLOB '*[^0-9a-f]*'),
                game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
                round INTEGER NOT NULL CHECK(round >= 1),
                user_id INTEGER NOT NULL REFERENCES users(id),
                request_id TEXT NOT NULL CHECK(length(request_id) = 32 AND request_id NOT GLOB '*[^0-9a-f]*'),
                seq INTEGER NOT NULL CHECK(seq >= 1),
                video_id TEXT NOT NULL, title TEXT NOT NULL, author_name TEXT NOT NULL,
                thumbnail_url TEXT NOT NULL, embed_url TEXT NOT NULL,
                submitted_at INTEGER NOT NULL,
                answer TEXT,
                play_round INTEGER CHECK(play_round IS NULL OR play_round >= 1),
                UNIQUE(game_id, user_id, request_id), UNIQUE(game_id, seq)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS game_ballot (
                submission_id TEXT PRIMARY KEY NOT NULL REFERENCES game_submissions(id) ON DELETE CASCADE,
                game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
                round INTEGER NOT NULL,
                group_index INTEGER NOT NULL CHECK(group_index >= 0)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS game_votes (
                submission_id TEXT NOT NULL REFERENCES game_ballot(submission_id) ON DELETE CASCADE,
                user_id INTEGER NOT NULL REFERENCES users(id),
                game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
                round INTEGER NOT NULL,
                PRIMARY KEY(submission_id, user_id)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS game_plays (
                game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
                seq INTEGER NOT NULL CHECK(seq >= 1),
                round INTEGER NOT NULL,
                submission_id TEXT NOT NULL REFERENCES game_submissions(id) ON DELETE CASCADE,
                started_at INTEGER,
                ends_at INTEGER CHECK(ends_at IS NULL OR started_at IS NOT NULL),
                finished INTEGER NOT NULL CHECK(finished IN (0, 1)),
                PRIMARY KEY(game_id, seq)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS game_scores (
                game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
                user_id INTEGER NOT NULL REFERENCES users(id),
                points INTEGER NOT NULL,
                PRIMARY KEY(game_id, user_id)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS game_guesses (
                game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
                round INTEGER NOT NULL,
                user_id INTEGER NOT NULL REFERENCES users(id),
                wrong INTEGER NOT NULL DEFAULT 0 CHECK(wrong >= 0),
                rank INTEGER CHECK(rank IS NULL OR rank >= 1),
                points INTEGER NOT NULL DEFAULT 0,
                correct_at INTEGER,
                CHECK((rank IS NULL) = (correct_at IS NULL)),
                PRIMARY KEY(game_id, round, user_id)
            ) STRICT",
            "CREATE TABLE IF NOT EXISTS game_images (
                id TEXT PRIMARY KEY NOT NULL CHECK(length(id) = 32 AND id NOT GLOB '*[^0-9a-f]*'),
                lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
                content_type TEXT NOT NULL CHECK(content_type IN ('image/jpeg', 'image/png')),
                width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 4096),
                height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 4096),
                bytes INTEGER NOT NULL CHECK(bytes >= 1),
                created_at INTEGER NOT NULL
            ) STRICT",
            "CREATE INDEX IF NOT EXISTS game_images_lobby ON game_images(lobby_id)",
            "CREATE INDEX IF NOT EXISTS game_submissions_round ON game_submissions(game_id, round)",
            "CREATE INDEX IF NOT EXISTS game_votes_round ON game_votes(game_id, round)",
        ] {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        // Databases created by earlier milestones lack these columns.
        for (table, column, definition) in [
            ("game_submissions", "answer", "answer TEXT"),
            (
                "game_submissions",
                "play_round",
                "play_round INTEGER CHECK(play_round IS NULL OR play_round >= 1)",
            ),
            ("games", "prompts", "prompts TEXT NOT NULL DEFAULT '[]'"),
        ] {
            let present: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?) WHERE name = ?)",
            )
            .bind(table)
            .bind(column)
            .fetch_one(&mut *tx)
            .await?;
            if !present {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "ALTER TABLE {table} ADD COLUMN {definition}"
                )))
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await
    }
}

impl Database {
    /// Host check before an upload body is read or decoded.
    pub(crate) async fn require_game_host(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        clock: impl Fn() -> i64,
    ) -> Result<(), GameError> {
        let mut tx = self.db_pool.begin().await?;
        let bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        Ok(())
    }

    /// Stores a processed upload: the file is written inside the transaction and
    /// removed again if the row does not commit.
    pub(crate) async fn store_game_image(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        image: &Processed,
        clock: impl Fn() -> i64,
    ) -> Result<GameImage, GameError> {
        let root = images::root(self).await?;
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM game_images WHERE lobby_id = ?")
            .bind(&bound.lobby_id)
            .fetch_one(&mut *tx)
            .await?;
        if count >= images::MAX_IMAGES_PER_LOBBY {
            return Err(GameError::ImageLimit);
        }
        let id = random_id()?;
        sqlx::query("INSERT INTO game_images(id, lobby_id, content_type, width, height, bytes, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&id).bind(&bound.lobby_id).bind(image.content_type)
            .bind(i64::from(image.width)).bind(i64::from(image.height)).bind(image.bytes.len() as i64).bind(clock())
            .execute(&mut *tx).await?;
        check_deadlines(&bound, user, clock())?;
        let file = images::path(&root, &bound.lobby_id, &id, image.content_type);
        let written = async {
            tokio::fs::create_dir_all(images::lobby_dir(&root, &bound.lobby_id)).await?;
            tokio::fs::write(&file, &image.bytes).await
        }
        .await;
        if let Err(error) = written {
            eprintln!("game image could not be written: {error}");
            let _ = tokio::fs::remove_file(&file).await;
            return Err(GameError::Unavailable);
        }
        if let Err(error) = tx.commit().await {
            let _ = tokio::fs::remove_file(&file).await;
            return Err(error.into());
        }
        Ok(GameImage {
            image_id: id,
            content_type: image.content_type.into(),
            width: image.width.into(),
            height: image.height.into(),
            bytes: image.bytes.len() as i64,
        })
    }

    pub(crate) async fn list_game_images(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        clock: impl Fn() -> i64,
    ) -> Result<GameImagesResponse, GameError> {
        let mut tx = self.db_pool.begin().await?;
        let bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        let rows: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
            "SELECT id, content_type, width, height, bytes FROM game_images WHERE lobby_id = ? ORDER BY created_at, id",
        )
        .bind(&bound.lobby_id)
        .fetch_all(&mut *tx)
        .await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        Ok(GameImagesResponse {
            images: rows
                .into_iter()
                .map(|(image_id, content_type, width, height, bytes)| GameImage {
                    image_id,
                    content_type,
                    width,
                    height,
                    bytes,
                })
                .collect(),
            limit: images::MAX_IMAGES_PER_LOBBY,
        })
    }

    /// The file for an image the caller may see: any lobby image for the host,
    /// otherwise only images of prompts the game has reached.
    pub(crate) async fn game_image(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        image: &str,
        clock: impl Fn() -> i64,
    ) -> Result<(String, std::path::PathBuf), GameError> {
        let root = images::root(self).await?;
        let mut tx = self.db_pool.begin().await?;
        let bound = authorize(&mut tx, user, membership, Permission::Member, &clock).await?;
        let content_type: Option<String> = sqlx::query_scalar(
            "SELECT content_type FROM game_images WHERE id = ? AND lobby_id = ?",
        )
        .bind(image)
        .bind(&bound.lobby_id)
        .fetch_optional(&mut *tx)
        .await?;
        let content_type = content_type.ok_or(GameError::ImageUnavailable)?;
        if user.id != bound.owner {
            let game = load(&mut tx, &bound.lobby_id).await?;
            if !game.is_some_and(|g| revealed_images(&g).any(|id| id == image)) {
                return Err(GameError::ImageUnavailable);
            }
        }
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        let file = images::path(&root, &bound.lobby_id, image, &content_type);
        Ok((content_type, file))
    }

    /// Deletes an image unless a running game still needs it. Absent IDs are no-ops.
    pub(crate) async fn delete_game_image(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        image: &str,
        clock: impl Fn() -> i64,
    ) -> Result<(), GameError> {
        let root = images::root(self).await?;
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let bound = authorize(&mut tx, user, membership, Permission::Host, &clock).await?;
        if let Some(game) = load(&mut tx, &bound.lobby_id).await? {
            if game.status == GameStatus::Running
                && game
                    .prompts
                    .iter()
                    .any(|p| p.image_id.as_deref() == Some(image))
            {
                return Err(GameError::ImageInUse);
            }
        }
        let content_type: Option<String> = sqlx::query_scalar(
            "DELETE FROM game_images WHERE id = ? AND lobby_id = ? RETURNING content_type",
        )
        .bind(image)
        .bind(&bound.lobby_id)
        .fetch_optional(&mut *tx)
        .await?;
        check_deadlines(&bound, user, clock())?;
        tx.commit().await?;
        if let Some(content_type) = content_type {
            let _ =
                tokio::fs::remove_file(images::path(&root, &bound.lobby_id, image, &content_type))
                    .await;
        }
        Ok(())
    }

    /// Removes image folders of lobbies that are closed or gone. Runs after a host
    /// closes a lobby and on the cleanup interval (which deletes expired lobbies).
    pub(crate) async fn sweep_game_images(&self) -> Result<usize, sqlx::Error> {
        let root = images::root(self).await?;
        let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
            return Ok(0);
        };
        let mut removed = 0;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !crate::models::queue::valid_id(&name) {
                continue;
            }
            let open: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM lobbies WHERE id = ? AND closed_at IS NULL)",
            )
            .bind(&name)
            .fetch_one(&self.db_pool)
            .await?;
            if !open && tokio::fs::remove_dir_all(entry.path()).await.is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// The curated starter title when the song has one, else the cleaned YouTube title.
/// Either way `accepts` treats bracketed parts and "ft." tails as optional, so the
/// starter title "Old Town Road (Remix)" also takes "Old Town Road".
async fn answer_key(tx: &mut Tx<'_>, video: &VideoMetadata) -> Result<String, GameError> {
    let starter: Option<String> = sqlx::query_scalar(
        "SELECT m.title FROM catalog_metadata m JOIN catalog_songs s ON s.id = m.song_id
         WHERE s.video_id = ? AND m.source = 'starter'",
    )
    .bind(&video.video_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(starter.unwrap_or_else(|| clean_title(&video.title)))
}

async fn receipt(
    tx: &mut Tx<'_>,
    game: &Game,
    user: i64,
    request: &str,
    video: &str,
) -> Result<Option<String>, GameError> {
    let prior: Option<(String, String)> = sqlx::query_as(
        "SELECT id, video_id FROM game_submissions WHERE game_id = ? AND user_id = ? AND request_id = ?",
    )
    .bind(&game.id)
    .bind(user)
    .bind(request)
    .fetch_optional(&mut **tx)
    .await?;
    prior
        .map(|(id, old)| {
            if old == video {
                Ok(id)
            } else {
                Err(GameError::IdempotencyConflict)
            }
        })
        .transpose()
}
