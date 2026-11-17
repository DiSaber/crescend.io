use super::*;
use crate::game::GameError;

#[tokio::test]
async fn party_round_collects_votes_scores_and_overlaps_the_next_round() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    let (b, bm) = &lobby.guests[1];
    let started = f.start(&lobby, "party").await;
    let game = started.game.unwrap();
    assert_eq!(
        (game.round, game.phase, game.phase_seq),
        (1, Phase::Collect, 1)
    );
    assert_eq!(game.total_rounds, Some(5));
    assert_eq!(
        game.phase_ends_at.unwrap().timestamp(),
        NOW + game.mode.settings.collect_seconds
    );

    let host_pick = f
        .submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    let a_pick = f.submit(a, am, "guestaaa01").await.unwrap();
    // Collect is private: each caller sees only their own song plus a count.
    let seen = f.view(b, bm).await;
    assert!(seen.my_submissions.is_empty());
    assert_eq!(seen.submission_count, 2);
    assert_eq!(seen.phase, Phase::Collect);
    let a_view = f.view(a, am).await;
    assert_eq!(a_view.my_submissions.len(), 1);
    assert_eq!(a_view.my_submissions[0].id, a_pick.submission_id);

    // The last member's song closes collect early.
    let b_pick = f.submit(b, bm, "guestbbb01").await.unwrap();
    let game = b_pick.game.game.unwrap();
    assert_eq!(game.phase, Phase::Vote);
    let ballot = game.ballot.unwrap();
    assert_eq!(ballot.groups.len(), 1);
    assert_eq!(ballot.groups[0].entries.len(), 3);
    let own = ballot.groups[0].entries.iter().find(|e| e.mine).unwrap();
    assert_eq!(own.submission_id, b_pick.submission_id);
    assert!(!own.votable);
    // Ballots never name submitters or carry counts.
    let json = serde_json::to_value(&ballot).unwrap().to_string();
    assert!(!json.contains("submitted_by") && !json.contains("\"votes\""));

    assert!(matches!(
        f.vote(b, bm, &b_pick.submission_id).await,
        Err(GameError::SelfVote)
    ));
    assert!(f.vote(b, bm, &a_pick.submission_id).await.unwrap().changed);
    assert!(!f.vote(b, bm, &a_pick.submission_id).await.unwrap().changed);
    assert!(matches!(
        f.vote(b, bm, &host_pick.submission_id).await,
        Err(GameError::VoteLimit)
    ));
    f.vote(&lobby.host, &lobby.host_membership, &a_pick.submission_id)
        .await
        .unwrap();
    // The final voter closes the vote; with overlap, round 2 collects while A's song plays.
    let closed = f.vote(a, am, &host_pick.submission_id).await.unwrap();
    let game = closed.game.game.unwrap();
    assert_eq!((game.round, game.phase), (2, Phase::Collect));
    assert!(game.ballot.is_none());
    let playing = game.now_playing.unwrap();
    assert_eq!(playing.song.video_id, "guestaaa01");
    assert_eq!(playing.submitted_by_user_id, a.id.to_string());
    assert_eq!(playing.ends_at, None, "Party plays full songs");
    let result = game.last_result.unwrap();
    assert_eq!(result.round, 1);
    assert!(result.entries[0].winner);
    assert_eq!(result.entries[0].votes, 2);
    assert_eq!(result.entries[0].points, 20);
    let scores: Vec<(String, i64)> = game
        .scoreboard
        .iter()
        .map(|s| (s.user_id.clone(), s.points))
        .collect();
    assert_eq!(scores[0], (a.id.to_string(), 20));
    assert_eq!(scores[1], (lobby.host.id.to_string(), 10));
    assert_eq!(scores[2], (b.id.to_string(), 0));
}

#[tokio::test]
async fn ticker_advances_phases_on_the_injected_clock() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    let first = f.submit(a, am, "guestaaa01").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    assert!(f.tick().await.is_empty(), "nothing is due yet");
    let before = f.revision(&lobby).await;
    f.advance(44);
    assert!(f.tick().await.is_empty());
    f.advance(1);
    let published = f.tick().await;
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].1, before + 1);
    assert_eq!(f.revision(&lobby).await, before + 1);
    let game = f.view(a, am).await;
    assert_eq!(game.phase, Phase::Vote);
    assert_eq!(game.phase_ends_at.unwrap().timestamp(), f.now() + 30);

    // Nobody votes: the timer closes the vote and the earliest song wins the tie.
    f.advance(30);
    f.tick().await;
    let game = f.view(a, am).await;
    assert_eq!((game.round, game.phase), (2, Phase::Collect));
    assert_eq!(
        game.now_playing.as_ref().unwrap().song.video_id,
        "guestaaa01"
    );
    assert_eq!(
        game.last_result.unwrap().entries[0].submission_id,
        first.submission_id
    );

    // A full song holds round 2's play phase until the host reports it ended.
    f.advance(45);
    f.tick().await;
    let game = f.view(a, am).await;
    assert_eq!(
        game.phase,
        Phase::Play,
        "round 2 had no songs; round 1 still plays"
    );
    assert_eq!(game.round, 2);
    let seq = game.now_playing.unwrap().play_seq;
    let request = GameAdvanceRequest {
        phase_seq: None,
        play_seq: Some(seq + 1),
    };
    assert!(matches!(
        f.database
            .advance_game(&lobby.host, &lobby.host_membership, &request, || f.now())
            .await,
        Err(GameError::SongChanged)
    ));
    let request = GameAdvanceRequest {
        phase_seq: None,
        play_seq: Some(seq),
    };
    let game = f
        .database
        .advance_game(&lobby.host, &lobby.host_membership, &request, || f.now())
        .await
        .unwrap()
        .game
        .game
        .unwrap();
    assert_eq!((game.round, game.phase), (3, Phase::Collect));
    assert!(game.now_playing.is_none());
    // A late duplicate ended callback cannot skip anything else.
    assert!(matches!(
        f.database
            .advance_game(&lobby.host, &lobby.host_membership, &request, || f.now())
            .await,
        Err(GameError::SongChanged)
    ));
}

#[tokio::test]
async fn host_skips_phases_until_the_game_ends_with_a_kept_scoreboard() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    let skip = |seq| GameAdvanceRequest {
        phase_seq: Some(seq),
        play_seq: None,
    };
    assert!(matches!(
        f.database
            .advance_game(guest, gm, &skip(1), || f.now())
            .await,
        Err(GameError::HostRequired)
    ));
    assert!(matches!(
        f.database
            .advance_game(&lobby.host, &lobby.host_membership, &skip(9), || f.now())
            .await,
        Err(GameError::PhaseChanged)
    ));
    // Empty rounds are skipped straight to the next collect.
    for round in 1..=5 {
        let game = f.view(guest, gm).await;
        assert_eq!((game.round, game.phase), (round, Phase::Collect));
        f.database
            .advance_game(
                &lobby.host,
                &lobby.host_membership,
                &skip(game.phase_seq),
                || f.now(),
            )
            .await
            .unwrap();
    }
    let game = f.view(guest, gm).await;
    assert_eq!((game.status, game.phase), (GameStatus::Ended, Phase::Ended));
    assert_eq!(game.scoreboard.len(), 2);
    assert!(matches!(
        f.submit(guest, gm, "guestaaa01").await,
        Err(GameError::NotRunning)
    ));
    assert!(f.tick().await.is_empty(), "ended games never tick");
    // DELETE on an ended game dismisses it; the lobby is back to the jukebox.
    let dismissed = f
        .database
        .stop_game(&lobby.host, &lobby.host_membership, || f.now())
        .await
        .unwrap();
    assert!(dismissed.changed && dismissed.game.game.is_none());
    let again = f
        .database
        .stop_game(&lobby.host, &lobby.host_membership, || f.now())
        .await
        .unwrap();
    assert!(!again.changed);
}

#[tokio::test]
async fn stopping_keeps_scores_and_a_new_start_replaces_the_old_game() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    let first = f.start(&lobby, "party").await.game.unwrap().id;
    assert!(matches!(
        f.database
            .start_game(&lobby.host, &lobby.host_membership, "party", || f.now())
            .await,
        Err(GameError::GameRunning)
    ));
    assert!(matches!(
        f.database.start_game(guest, gm, "party", || f.now()).await,
        Err(GameError::HostRequired)
    ));
    assert!(matches!(
        f.database
            .start_game(&lobby.host, &lobby.host_membership, "nope", || f.now())
            .await,
        Err(GameError::ModeUnavailable)
    ));
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    let game = f.view(guest, gm).await;
    let host_song = game.ballot.unwrap().groups[0]
        .entries
        .iter()
        .find(|e| !e.mine)
        .unwrap()
        .submission_id
        .clone();
    f.vote(guest, gm, &host_song).await.unwrap();
    let stopped = f
        .database
        .stop_game(&lobby.host, &lobby.host_membership, || f.now())
        .await
        .unwrap();
    let game = stopped.game.game.unwrap();
    assert_eq!(game.status, GameStatus::Ended);
    assert!(game.now_playing.is_none());
    let second = f.start(&lobby, "party").await.game.unwrap();
    assert_ne!(second.id, first);
    assert!(second.scoreboard.iter().all(|s| s.points == 0));
    let orphans: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM game_submissions) + (SELECT count(*) FROM game_votes) + (SELECT count(*) FROM game_scores)",
    )
    .fetch_one(&f.database.db_pool)
    .await
    .unwrap();
    assert_eq!(orphans, 0, "old game rows are deleted with it");
}

#[tokio::test]
async fn submissions_are_idempotent_limited_and_phase_bound() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (a, am) = &lobby.guests[0];
    assert!(matches!(
        f.submit(a, am, "guestaaa01").await,
        Err(GameError::NotRunning)
    ));
    f.start(&lobby, "party").await;
    let first = f.submit(a, am, "guestaaa01").await.unwrap();
    assert!(first.changed);
    let retry = f.submit(a, am, "guestaaa01").await.unwrap();
    assert!(!retry.changed);
    assert_eq!(retry.submission_id, first.submission_id);
    let conflict = f
        .database
        .submit_game(
            a,
            am,
            &request_id(a, "guestaaa01"),
            &song("otherone01"),
            || f.now(),
        )
        .await;
    assert!(matches!(conflict, Err(GameError::IdempotencyConflict)));
    assert!(matches!(
        f.submit(a, am, "guestaaa02").await,
        Err(GameError::SubmissionLimit)
    ));
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    f.advance(45);
    f.tick().await;
    let (b, bm) = &lobby.guests[1];
    assert!(matches!(
        f.submit(b, bm, "guestbbb01").await,
        Err(GameError::PhaseClosed)
    ));
    // A retry of an accepted song still answers after collect closes.
    assert!(!f.submit(a, am, "guestaaa01").await.unwrap().changed);
    assert_eq!(f.view(a, am).await.phase, Phase::Vote);
}

#[tokio::test]
async fn a_late_submission_lands_in_the_round_that_is_open_after_catch_up() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (a, am) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    f.submit(a, am, "guestaaa01").await.unwrap();
    // The ticker has not run yet; the write itself catches the game up first.
    f.advance(45);
    let late = f
        .submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    let game = late.game.game.unwrap();
    assert_eq!((game.round, game.phase), (2, Phase::Collect));
    assert_eq!(game.my_submissions[0].id, late.submission_id);
    assert_eq!(game.now_playing.unwrap().song.video_id, "guestaaa01");
}

#[tokio::test]
async fn failed_revision_rolls_back_game_writes_and_membership_is_enforced() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    let outsider = f.account("outsider").await;
    assert!(matches!(
        f.database.game_view(&outsider, gm, || f.now()).await,
        Err(GameError::MembershipForbidden)
    ));
    sqlx::query("CREATE TRIGGER fail_game_revision BEFORE UPDATE OF revision ON lobbies BEGIN SELECT RAISE(ABORT, 'injected rollback'); END")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert!(f.submit(guest, gm, "guestaaa01").await.is_err());
    f.advance(45);
    assert!(f.database.tick_games(f.now()).await.is_err());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM game_submissions")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let phase: String = sqlx::query_scalar("SELECT phase FROM games")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(phase, "collect");
}

#[tokio::test]
async fn closing_or_expiring_the_lobby_deletes_its_game() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    f.database
        .leave_lobby(lobby.host.id, &lobby.host_membership, || f.now())
        .await
        .unwrap();
    let count = |table: &'static str| {
        let pool = f.database.db_pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table}"
            )))
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(count("games").await, 0);
    assert_eq!(count("game_submissions").await, 0);

    let other = f.lobby(0).await;
    f.start(&other, "party").await;
    f.advance(86_400);
    assert!(f.tick().await.is_empty(), "expired lobbies never tick");
    f.database
        .delete_expired_lobbies(crate::models::queue::timestamp(f.now()).unwrap())
        .await
        .unwrap();
    assert_eq!(count("games").await, 0);
}

#[tokio::test]
async fn host_skipping_a_held_play_phase_ends_the_song_that_holds_it() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    f.start(&lobby, "party").await;
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    f.advance(45);
    f.tick().await;
    // One song won round 1 and plays; round 2 collects meanwhile.
    let game = f.view(guest, gm).await;
    assert_eq!((game.round, game.phase), (2, Phase::Collect));
    f.submit(guest, gm, "guestaaa02").await.unwrap();
    f.advance(45);
    f.tick().await;
    let game = f.view(guest, gm).await;
    assert_eq!(
        game.phase,
        Phase::Play,
        "round 2's winner waits for round 1's song"
    );
    assert_eq!(game.up_next_count, 1);
    assert_eq!(game.now_playing.as_ref().unwrap().round, 1);
    let skip = GameAdvanceRequest {
        phase_seq: Some(game.phase_seq),
        play_seq: None,
    };
    let game = f
        .database
        .advance_game(&lobby.host, &lobby.host_membership, &skip, || f.now())
        .await
        .unwrap()
        .game
        .game
        .unwrap();
    assert_eq!((game.round, game.phase), (3, Phase::Collect));
    let playing = game.now_playing.unwrap();
    assert_eq!(
        (playing.round, playing.song.video_id.as_str()),
        (2, "guestaaa02")
    );
    assert_eq!(playing.started_at.timestamp(), f.now());
}
