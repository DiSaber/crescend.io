//! G6 lifecycle hardening: the host or every guest leaving mid-game, the lobby
//! expiring mid-round, double starts, and timers exactly at phase boundaries.
use super::*;
use crate::game::{GameError, rules::*};

/// Tokens outlive the day-long lobby so expiry, not auth, is what the test sees.
fn long_lived(user: &AuthenticatedUser) -> AuthenticatedUser {
    AuthenticatedUser {
        id: user.id,
        expires_at: NOW + 10 * 86_400,
    }
}

async fn leave(f: &Fixture, user: &AuthenticatedUser, membership: &str) {
    f.database
        .leave_lobby(user.id, membership, || f.now())
        .await
        .unwrap()
        .expect("an open lobby publishes the leave");
}

async fn count(f: &Fixture, table: &'static str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap()
}

async fn skip_seq(f: &Fixture, lobby: &Lobby, seq: i64) -> Result<GameView, GameError> {
    let request = GameAdvanceRequest {
        phase_seq: Some(seq),
        play_seq: None,
    };
    Ok(f.database
        .advance_game(&lobby.host, &lobby.host_membership, &request, || f.now())
        .await?
        .game
        .game
        .unwrap())
}

async fn propose(
    f: &Fixture,
    user: &AuthenticatedUser,
    membership: &str,
    video: &str,
    title: &str,
) {
    let mut metadata = song(video);
    metadata.title = title.into();
    f.database
        .submit_game(
            user,
            membership,
            &request_id(user, video),
            &metadata,
            || f.now(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn host_leaving_mid_vote_ends_the_game_for_everyone() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    let pick = f.submit(a, am, "guestaaa01").await.unwrap();
    f.submit(b, bm, "guestbbb01").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    assert_eq!(f.view(a, am).await.phase, Phase::Vote);
    f.vote(b, bm, &pick.submission_id).await.unwrap();

    leave(&f, &lobby.host, &lobby.host_membership).await;
    for table in [
        "games",
        "game_submissions",
        "game_ballot",
        "game_votes",
        "game_scores",
    ] {
        assert_eq!(count(&f, table).await, 0, "{table} is cleared");
    }
    // Guests lose the game with the lobby: reads and every write are refused.
    assert!(matches!(
        f.database.game_view(a, am, || f.now()).await,
        Err(GameError::MembershipForbidden)
    ));
    assert!(matches!(
        f.vote(a, am, &pick.submission_id).await,
        Err(GameError::MembershipForbidden)
    ));
    assert!(matches!(
        f.submit(b, bm, "guestbbb02").await,
        Err(GameError::MembershipForbidden)
    ));
    // The ticker has nothing left to advance, even once the vote would have closed.
    f.advance(30);
    assert!(f.tick().await.is_empty());
}

#[tokio::test]
async fn host_leaving_during_a_tune_guess_hides_the_answer_for_good() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    f.start(&lobby, "name_that_tune").await;
    propose(&f, a, am, "fJ9rUzIMcZQ", "Queen - Bohemian Rhapsody").await;
    f.advance(90);
    f.tick().await;
    assert_eq!(f.view(b, bm).await.phase, Phase::Guess);
    leave(&f, &lobby.host, &lobby.host_membership).await;
    assert!(matches!(
        f.database
            .guess_game(b, bm, "bohemian rhapsody", || f.now())
            .await,
        Err(GameError::MembershipForbidden)
    ));
    assert_eq!(count(&f, "game_guesses").await, 0);
    assert!(f.tick().await.is_empty());
}

#[tokio::test]
async fn lobby_expiring_mid_round_freezes_and_then_deletes_the_game() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let host = long_lived(&lobby.host);
    let (guest, gm) = &lobby.guests[0];
    let guest = long_lived(guest);
    // Start 20 seconds before the lobby's day is up: the collect timer (45 s) outlives it.
    f.advance(86_400 - 20);
    f.database
        .start_game(
            &host,
            &lobby.host_membership,
            "party",
            &(crate::game::modes::preset("party").unwrap().definition)(),
            &[],
            || f.now(),
        )
        .await
        .unwrap();
    f.database
        .submit_game(
            &guest,
            gm,
            &request_id(&guest, "guestaaa01"),
            &song("guestaaa01"),
            || f.now(),
        )
        .await
        .unwrap();
    f.advance(19);
    assert_eq!(
        f.database
            .game_view(&guest, gm, || f.now())
            .await
            .unwrap()
            .game
            .unwrap()
            .phase,
        Phase::Collect,
        "one second before expiry the round is still open"
    );
    f.advance(1);
    // At the expiry instant the lobby is gone for reads, writes and the ticker.
    assert!(matches!(
        f.database.game_view(&guest, gm, || f.now()).await,
        Err(GameError::MembershipForbidden)
    ));
    assert!(matches!(
        f.database
            .submit_game(
                &host,
                &lobby.host_membership,
                &request_id(&host, "hosthost01"),
                &song("hosthost01"),
                || f.now()
            )
            .await,
        Err(GameError::MembershipForbidden)
    ));
    let before = f.revision(&lobby).await;
    f.advance(60);
    assert!(f.tick().await.is_empty(), "expired lobbies never tick");
    assert_eq!(f.revision(&lobby).await, before, "nothing is published");
    let game: (String, i64) = sqlx::query_as("SELECT phase, round FROM games")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(
        game,
        ("collect".into(), 1),
        "the stored game froze mid-round"
    );
    f.database
        .delete_expired_lobbies(crate::models::queue::timestamp(f.now()).unwrap())
        .await
        .unwrap();
    assert_eq!(count(&f, "games").await, 0);
    assert_eq!(count(&f, "game_submissions").await, 0);
}

#[tokio::test]
async fn every_guest_leaving_mid_collect_lets_the_host_finish_alone() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    f.submit(a, am, "guestaaa01").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    leave(&f, a, am).await;
    // B never picked: their leaving completes collect at once instead of at the timer.
    let before = f.revision(&lobby).await;
    leave(&f, b, bm).await;
    assert_eq!(
        f.revision(&lobby).await,
        before + 1,
        "one commit for leave + settle"
    );
    let game = f.view(&lobby.host, &lobby.host_membership).await;
    assert_eq!(
        (game.round, game.phase),
        (1, Phase::Vote),
        "two songs went to a vote without waiting 45 s"
    );
    let ballot = game.ballot.unwrap();
    assert_eq!(
        ballot.groups[0].entries.len(),
        2,
        "a departed player's pick stays"
    );
    // The host is the only voter left; their vote closes the round.
    let theirs = ballot.groups[0].entries.iter().find(|e| !e.mine).unwrap();
    let game = f
        .vote(&lobby.host, &lobby.host_membership, &theirs.submission_id)
        .await
        .unwrap()
        .game
        .game
        .unwrap();
    assert_eq!((game.round, game.phase), (2, Phase::Collect));
    let a_row = game
        .scoreboard
        .iter()
        .find(|row| row.user_id == a.id.to_string())
        .unwrap();
    assert!(
        !a_row.active,
        "departed players stay on the board, marked inactive"
    );
    assert_eq!(a_row.points, 10, "the host's one vote still scores");
    // Alone, the host can play on to the end.
    let mut game = game;
    for _ in 0..40 {
        if game.status == GameStatus::Ended {
            break;
        }
        game = skip_seq(&f, &lobby, game.phase_seq).await.unwrap();
    }
    assert_eq!(game.status, GameStatus::Ended);
    // Departed players stay listed only when they scored.
    assert_eq!(game.scoreboard.len(), 2);
}

#[tokio::test]
async fn a_guest_leaving_without_voting_closes_the_vote() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    let pa = f.submit(a, am, "guestaaa01").await.unwrap();
    let pb = f.submit(b, bm, "guestbbb01").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    f.vote(&lobby.host, &lobby.host_membership, &pb.submission_id)
        .await
        .unwrap();
    f.vote(a, am, &pb.submission_id).await.unwrap();
    leave(&f, b, bm).await;
    let game = f.view(a, am).await;
    assert_eq!((game.round, game.phase), (2, Phase::Collect));
    let result = game.last_result.unwrap();
    assert!(result.entries[0].winner);
    assert_eq!(result.entries[0].submission_id, pb.submission_id);
    assert_ne!(result.entries[0].submission_id, pa.submission_id);
}

#[tokio::test]
async fn every_guesser_leaving_reveals_the_tune_and_the_game_still_ends() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    f.start(&lobby, "name_that_tune").await;
    propose(&f, a, am, "fJ9rUzIMcZQ", "Queen - Bohemian Rhapsody").await;
    propose(
        &f,
        a,
        am,
        "hTWKbfoikeg",
        "Nirvana - Smells Like Teen Spirit",
    )
    .await;
    f.advance(90);
    f.tick().await;
    assert_eq!(f.view(a, am).await.phase, Phase::Guess);
    // B is the only guesser (A proposed it, the host plays it): B leaving ends the guess.
    leave(&f, b, bm).await;
    let game = f.view(a, am).await;
    assert_eq!((game.round, game.phase), (1, Phase::Reveal));
    assert!(game.last_result.unwrap().guesses.is_empty());
    // A leaves too. Round 2 has no guessers at all, so it only waits for the reveals.
    leave(&f, a, am).await;
    f.advance(8);
    f.tick().await;
    let game = f.view(&lobby.host, &lobby.host_membership).await;
    assert_eq!((game.round, game.phase), (2, Phase::Reveal));
    f.advance(8);
    f.tick().await;
    let game = f.view(&lobby.host, &lobby.host_membership).await;
    assert_eq!(game.status, GameStatus::Ended);
    assert!(f.tick().await.is_empty());
}

#[tokio::test]
async fn starting_twice_is_refused_and_concurrent_starts_make_one_game() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    assert!(matches!(
        f.start_with(&lobby, "duel", serde_json::json!({})).await,
        Err(GameError::GameRunning)
    ));
    assert_eq!(
        f.view(guest, gm).await.mode_id,
        "party",
        "the running game is untouched"
    );
    assert_eq!(f.view(guest, gm).await.submission_count, 1);

    // A restart after ending starts clean: no picks or scores carry over.
    f.database
        .stop_game(&lobby.host, &lobby.host_membership, || f.now())
        .await
        .unwrap();
    let game = f.start(&lobby, "party").await.game.unwrap();
    assert_eq!(
        (game.round, game.phase, game.submission_count),
        (1, Phase::Collect, 0)
    );
    assert!(game.my_submissions.is_empty() && game.last_result.is_none());
    assert_eq!(count(&f, "games").await, 1);

    // Two starts racing (a double tap, two host tabs): exactly one wins.
    let other = f.lobby(1).await;
    let results = futures_util::future::join_all(
        (0..4).map(|_| f.start_with(&other, "speed_round", serde_json::json!({}))),
    )
    .await;
    let won = results.iter().filter(|r| r.is_ok()).count();
    let refused = results
        .iter()
        .filter(|r| matches!(r, Err(GameError::GameRunning)))
        .count();
    assert_eq!((won, refused), (1, 3), "{results:?}");
    assert_eq!(count(&f, "games").await, 2);
}

#[tokio::test]
async fn http_double_start_returns_one_game_and_one_game_running() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (base, server) = f.http().await;
    let url = format!(
        "{base}/api/lobbies/memberships/{}/game",
        lobby.host_membership
    );
    let send = || {
        f.request(reqwest::Method::POST, url.clone(), &lobby.host)
            .json(&serde_json::json!({"mode_id": "party"}))
            .send()
    };
    let (one, two) = tokio::join!(send(), send());
    let mut statuses = [one.unwrap(), two.unwrap()];
    statuses.sort_by_key(|r| r.status());
    assert_eq!(statuses[0].status(), 200);
    assert_eq!(statuses[1].status(), 409);
    let [_, refused] = statuses;
    let error: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(error["code"], "game_running");
    server.abort();
}

#[tokio::test]
async fn votes_and_guesses_exactly_at_the_deadline_are_closed() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    let pa = f.submit(a, am, "guestaaa01").await.unwrap();
    let pb = f.submit(b, bm, "guestbbb01").await.unwrap();
    f.advance(45);
    f.tick().await;
    let vote = f.view(a, am).await;
    assert_eq!(vote.phase, Phase::Vote);
    let deadline = vote.phase_ends_at.unwrap().timestamp();
    // One second early still counts; at the deadline the ballot is closed.
    f.advance(deadline - f.now() - 1);
    assert!(f.vote(a, am, &pb.submission_id).await.unwrap().changed);
    f.advance(1);
    let late = f.vote(b, bm, &pa.submission_id).await;
    assert!(matches!(late, Err(GameError::PhaseClosed)), "{late:?}");
    // The refused write rolls back its catch-up; the ticker (1 s) publishes it.
    assert_eq!(f.tick().await.len(), 1);
    let game = f.view(a, am).await;
    let result = game
        .last_result
        .clone()
        .unwrap_or_else(|| panic!("{game:?}"));
    let winner = result.entries.iter().find(|e| e.winner).unwrap();
    assert_eq!(winner.submission_id, pb.submission_id);
    assert_eq!(winner.votes, 1, "the late vote was not counted");

    // Name That Tune: a correct guess at the guess deadline is too late.
    let tune = f.lobby(2).await;
    let (c, cm) = &tune.guests[0];
    let (d, dm) = &tune.guests[1];
    f.start(&tune, "name_that_tune").await;
    propose(&f, c, cm, "fJ9rUzIMcZQ", "Queen - Bohemian Rhapsody").await;
    f.advance(90);
    f.tick().await;
    let guess = f.view(d, dm).await;
    assert_eq!(guess.phase, Phase::Guess);
    f.advance(guess.phase_ends_at.unwrap().timestamp() - f.now());
    let late = f
        .database
        .guess_game(d, dm, "bohemian rhapsody", || f.now())
        .await;
    assert!(matches!(late, Err(GameError::PhaseClosed)), "{late:?}");
    f.tick().await;
    let game = f.view(d, dm).await;
    assert_eq!(game.phase, Phase::Reveal);
    assert!(game.last_result.unwrap().guesses.is_empty());
}

#[tokio::test]
async fn a_skip_racing_the_ticker_never_skips_two_phases() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    let vote = f.view(guest, gm).await;
    assert_eq!(vote.phase, Phase::Vote);
    // The host taps Skip just as the vote timer runs out, before the ticker runs.
    f.advance(vote.phase_ends_at.unwrap().timestamp() - f.now());
    assert!(matches!(
        skip_seq(&f, &lobby, vote.phase_seq).await,
        Err(GameError::PhaseChanged)
    ));
    // The refused skip changed nothing; the ticker closes the vote once.
    assert_eq!(f.tick().await.len(), 1);
    let after = f.view(guest, gm).await;
    assert_eq!((after.round, after.phase), (2, Phase::Collect));
    assert!(after.phase_seq > vote.phase_seq);
    assert_eq!(
        after.now_playing.as_ref().map(|p| p.round),
        Some(1),
        "round 1's winner plays; nothing was skipped"
    );
    assert!(f.tick().await.is_empty(), "nothing left for the ticker");
}

#[tokio::test]
async fn a_stalled_ticker_catches_up_in_one_commit_with_future_deadlines() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    f.start(&lobby, "name_that_tune").await;
    propose(&f, a, am, "fJ9rUzIMcZQ", "Queen - Bohemian Rhapsody").await;
    propose(
        &f,
        b,
        bm,
        "hTWKbfoikeg",
        "Nirvana - Smells Like Teen Spirit",
    )
    .await;
    f.advance(90);
    f.tick().await;
    let before = f.revision(&lobby).await;
    // The server was paused for two minutes in the middle of round 1's guess.
    f.advance(120);
    let published = f.tick().await;
    assert_eq!(published.len(), 1);
    assert_eq!(
        published[0].1,
        before + 1,
        "one revision for the whole catch-up"
    );
    let game = f.view(a, am).await;
    // Round 1 closed when the server woke; round 2 got a full timer from then.
    assert_eq!((game.round, game.phase), (1, Phase::Reveal));
    assert!(game.phase_ends_at.unwrap().timestamp() > f.now());
    f.advance(8);
    f.tick().await;
    let game = f.view(a, am).await;
    assert_eq!((game.round, game.phase), (2, Phase::Guess));
    assert_eq!(game.phase_ends_at.unwrap().timestamp(), f.now() + 30);
    let playing = game.now_playing.unwrap();
    assert_eq!(playing.round, 2);
    assert_eq!(
        playing.started_at.timestamp(),
        f.now(),
        "the clip starts when the round does"
    );

    // An hour-long stall closes only the overdue phase: the reveal that follows still
    // gets its full 8 seconds on screen instead of flashing past.
    f.advance(3_600);
    assert_eq!(f.tick().await.len(), 1);
    let game = f.view(a, am).await;
    assert_eq!((game.round, game.phase), (2, Phase::Reveal));
    assert_eq!(game.phase_ends_at.unwrap().timestamp(), f.now() + 8);
    f.advance(8);
    f.tick().await;
    let game = f.view(a, am).await;
    assert_eq!(game.status, GameStatus::Ended);
    assert_eq!(game.phase_ends_at, None);
    assert!(f.tick().await.is_empty());
}

#[tokio::test]
async fn timers_never_count_down_from_the_past_after_a_settle() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    let custom = crate::game::custom_definition(
        "Clip party",
        Rules {
            prompt: PromptRule::None,
            collect: CollectRule::Pick,
            ballot: BallotRule::All,
            winner: WinnerRule::MostVotes,
            challenge: ChallengeRule::Listen,
            scoring: ScoringRule::VotesToPicker,
        },
    )
    .unwrap();
    let mut overrides = serde_json::Map::new();
    overrides.insert("play_seconds".into(), 30.into());
    let definition = crate::game::customize(custom, &overrides).unwrap();
    f.database
        .start_game(
            &lobby.host,
            &lobby.host_membership,
            "custom",
            &definition,
            &[],
            || f.now(),
        )
        .await
        .unwrap();
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    // Every few seconds across several rounds, every deadline shown is ahead of now.
    for _ in 0..200 {
        f.advance(3);
        f.tick().await;
        let game = f.view(guest, gm).await;
        if game.status == GameStatus::Ended {
            break;
        }
        if let Some(at) = game.phase_ends_at {
            assert!(
                at.timestamp() > f.now(),
                "{:?} ends at {at} <= {}",
                game.phase,
                f.now()
            );
        }
        if let Some(ends) = game.now_playing.as_ref().and_then(|p| p.ends_at) {
            assert!(ends.timestamp() > f.now());
        }
        if game.phase == Phase::Collect && game.my_submissions.is_empty() {
            let video = format!("guest{:05}", game.round);
            f.submit(guest, gm, &video).await.unwrap();
        }
    }
}
