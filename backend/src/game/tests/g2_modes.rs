use super::*;
use crate::game::GameError;
use serde_json::json;

async fn submissions(f: &Fixture, lobby: &Lobby) -> Vec<String> {
    let mut ids = vec![
        f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
            .await
            .unwrap()
            .submission_id,
    ];
    for (i, (user, membership)) in lobby.guests.iter().enumerate() {
        ids.push(
            f.submit(user, membership, &format!("guest{i:05}"))
                .await
                .unwrap()
                .submission_id,
        );
    }
    ids
}

#[tokio::test]
async fn duel_pairs_songs_takes_one_vote_per_pair_and_plays_every_pair_winner() {
    let f = Fixture::new().await;
    let lobby = f.lobby(3).await;
    let started = f.start(&lobby, "duel").await.game.unwrap();
    assert_eq!(started.mode.name, "Duel");
    // Songs pair in submission order: [host, g0] and [g1, g2].
    let ids = submissions(&f, &lobby).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    let (g2, m2) = &lobby.guests[2];
    let ballot = f.view(g0, m0).await.ballot.unwrap();
    assert_eq!(ballot.groups.len(), 2);
    assert_eq!(ballot.groups[0].entries[0].submission_id, ids[0]);
    assert_eq!(ballot.votes_allowed, 1, "g0 cannot vote in its own pair");
    assert!(!ballot.groups[0].entries.iter().any(|e| e.votable));
    assert_eq!(
        f.view(&lobby.host, &lobby.host_membership)
            .await
            .ballot
            .unwrap()
            .votes_allowed,
        1
    );

    assert!(
        matches!(f.vote(g0, m0, &ids[0]).await, Err(GameError::SelfVote)),
        "own pair, other song"
    );
    f.vote(g0, m0, &ids[2]).await.unwrap();
    assert!(
        matches!(f.vote(g0, m0, &ids[3]).await, Err(GameError::VoteLimit)),
        "one vote per pair"
    );
    f.vote(&lobby.host, &lobby.host_membership, &ids[3])
        .await
        .unwrap();
    f.vote(g1, m1, &ids[0]).await.unwrap();
    let game = f.vote(g2, m2, &ids[1]).await.unwrap().game.game.unwrap();
    // Pair 1: host 1 vote vs g0 1 vote -> earliest (host). Pair 2: g1 1 vs g2 1 -> g1.
    assert_eq!((game.round, game.phase), (2, Phase::Collect));
    let playing = game.now_playing.unwrap();
    assert_eq!(playing.song.as_ref().unwrap().video_id, "hosthost01");
    assert_eq!(playing.ends_at.unwrap().timestamp(), f.now() + 60);
    assert_eq!(game.up_next_count, 1);
    let result = game.last_result.unwrap();
    let winners: Vec<_> = result
        .entries
        .iter()
        .filter(|e| e.winner)
        .map(|e| e.submission_id.clone())
        .collect();
    assert_eq!(winners, vec![ids[0].clone(), ids[2].clone()]);
    let points = |user: &AuthenticatedUser| {
        game.scoreboard
            .iter()
            .find(|s| s.user_id == user.id.to_string())
            .unwrap()
            .points
    };
    assert_eq!(points(&lobby.host), crate::game::rules::WIN_BONUS);
    assert_eq!(points(g1), crate::game::rules::WIN_BONUS);
    assert_eq!(points(g0), 0);

    // The server ends timed songs; the second pair winner follows.
    f.advance(60);
    f.tick().await;
    let game = f.view(g0, m0).await;
    let playing = game.now_playing.unwrap();
    assert_eq!(playing.song.as_ref().unwrap().video_id, "guest00001");
    assert_eq!(playing.started_at.timestamp(), f.now());
    assert_eq!(game.up_next_count, 0);
}

#[tokio::test]
async fn duel_gives_an_odd_song_a_bye_that_plays_but_scores_nothing() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    f.start(&lobby, "duel").await;
    let ids = submissions(&f, &lobby).await;
    let (g0, m0) = &lobby.guests[0];
    let ballot = f.view(g0, m0).await.ballot.unwrap();
    assert_eq!(ballot.groups[1].entries.len(), 1);
    assert!(!ballot.groups[1].entries[0].votable);
    assert!(matches!(
        f.vote(&lobby.host, &lobby.host_membership, &ids[2]).await,
        Err(GameError::VoteLimit)
    ));
    // Only the guests can vote in the pair (the host owns one side); the guest of the bye votes.
    let (g1, m1) = &lobby.guests[1];
    let game = f.vote(g1, m1, &ids[1]).await.unwrap().game.game.unwrap();
    assert_eq!(game.phase, Phase::Collect, "every possible vote was cast");
    let result = game.last_result.unwrap();
    assert_eq!(result.entries.iter().filter(|e| e.winner).count(), 2);
    assert!(
        game.scoreboard
            .iter()
            .all(|s| s.points == 0 || s.user_id == g0.id.to_string())
    );
}

#[tokio::test]
async fn speed_round_runs_on_short_timers_and_clips_end_without_the_host() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    let game = f.start(&lobby, "speed_round").await.game.unwrap();
    assert_eq!(game.phase_ends_at.unwrap().timestamp(), NOW + 20);
    assert_eq!(game.total_rounds, Some(8));
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap();
    assert_eq!(
        f.view(guest, gm).await.phase_ends_at.unwrap().timestamp(),
        NOW + 15
    );
    f.advance(15);
    f.tick().await;
    let game = f.view(guest, gm).await;
    assert_eq!(
        game.now_playing
            .as_ref()
            .unwrap()
            .ends_at
            .unwrap()
            .timestamp(),
        f.now() + 60
    );
    f.advance(60);
    f.tick().await;
    assert!(f.view(guest, gm).await.now_playing.is_none());
}

#[tokio::test]
async fn start_settings_are_validated_against_the_schema() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    for bad in [
        json!({"rounds": 99}),
        json!({"vote_seconds": 17}),
        json!({"unknown": 1}),
        json!({"self_vote": "yes"}),
    ] {
        assert!(
            matches!(
                f.start_with(&lobby, "party", bad.clone()).await,
                Err(GameError::InvalidSettings)
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        f.start_with(&lobby, "duel", json!({"votes_per_player": 2}))
            .await,
        Err(GameError::InvalidSettings)
    ));
    let game = f
        .start_with(&lobby, "party", json!({"rounds": 2, "collect_seconds": 10, "play_seconds": 30, "clip_start_seconds": 15}))
        .await
        .unwrap()
        .game
        .unwrap();
    assert_eq!(game.total_rounds, Some(2));
    assert_eq!(game.mode.settings.play_seconds, 30);
    assert_eq!(game.phase_ends_at.unwrap().timestamp(), NOW + 10);
    let (guest, gm) = &lobby.guests[0];
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    f.advance(10);
    f.tick().await;
    let game = f.view(guest, gm).await;
    assert_eq!(game.now_playing.unwrap().start_seconds, 15);
}

#[tokio::test]
async fn custom_rules_run_when_valid_and_are_rejected_when_they_conflict() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let resolve = |value: serde_json::Value| {
        crate::game::resolve(&serde_json::from_value::<StartGameRequest>(value).unwrap())
    };
    let rules = json!({"prompt":"none","collect":"pick","ballot":"pairs","winner":"most_votes","challenge":"listen","scoring":"votes_to_picker"});
    assert!(matches!(
        resolve(json!({"custom": {"name": "Odd", "rules": rules}})),
        Err(GameError::InvalidRules)
    ));
    assert!(matches!(
        resolve(json!({"mode_id": "party", "custom": {"name": "x", "rules": rules}})),
        Err(GameError::InvalidRequest)
    ));
    assert!(matches!(resolve(json!({})), Err(GameError::InvalidRequest)));
    let good = json!({"prompt":"none","collect":"pick","ballot":"pairs","winner":"pair_winners","challenge":"listen","scoring":"votes_to_picker"});
    assert!(matches!(
        resolve(json!({"custom": {"name": "   ", "rules": good}})),
        Err(GameError::InvalidRequest)
    ));
    let (mode_id, definition) = resolve(
        json!({"custom": {"name": " Vote Duel ", "rules": good}, "settings": {"rounds": 0}}),
    )
    .unwrap();
    assert_eq!(mode_id, "custom");
    assert_eq!(definition.name, "Vote Duel");
    let game = f
        .database
        .start_game(
            &lobby.host,
            &lobby.host_membership,
            &mode_id,
            &definition,
            &[],
            || f.now(),
        )
        .await
        .unwrap()
        .game
        .unwrap();
    assert_eq!(game.total_rounds, None, "rounds 0 runs until stopped");
    for round in 1..=12 {
        let game = f.view(&lobby.host, &lobby.host_membership).await;
        assert_eq!(game.round, round);
        f.advance(45);
        f.tick().await;
    }
    assert_eq!(
        f.view(&lobby.host, &lobby.host_membership).await.status,
        GameStatus::Running
    );
}

#[tokio::test]
async fn without_overlap_the_next_round_waits_for_the_winner_to_finish() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    f.start_with(
        &lobby,
        "party",
        json!({"overlap_next_round": false, "play_seconds": 30, "rounds": 1}),
    )
    .await
    .unwrap();
    f.submit(guest, gm, "guestaaa01").await.unwrap();
    f.advance(45);
    f.tick().await;
    let game = f.view(guest, gm).await;
    assert_eq!((game.round, game.phase), (1, Phase::Play));
    f.advance(29);
    assert!(f.tick().await.is_empty());
    f.advance(1);
    f.tick().await;
    let game = f.view(guest, gm).await;
    assert_eq!(
        game.status,
        GameStatus::Ended,
        "the last round ends after its song"
    );
}

#[tokio::test]
async fn http_lists_all_presets_and_maps_settings_errors() {
    let f = Fixture::new().await;
    let lobby = f.lobby(0).await;
    let (base, server) = f.http().await;
    let modes: serde_json::Value = f
        .request(
            reqwest::Method::GET,
            format!("{base}/api/game-modes"),
            &lobby.host,
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<_> = modes["modes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(ids.starts_with(&["party", "speed_round", "duel"]));
    let url = format!(
        "{base}/api/lobbies/memberships/{}/game",
        lobby.host_membership
    );
    for (body, code) in [
        (
            json!({"mode_id": "party", "settings": {"rounds": 0.5}}),
            "invalid_game_settings",
        ),
        (
            json!({"custom": {"name": "x", "rules": {"prompt":"none","collect":"pick","ballot":"all","winner":"pair_winners","challenge":"listen","scoring":"winner_bonus"}}}),
            "invalid_game_rules",
        ),
        (
            json!({"custom": {"name": "x", "rules": {"prompt":"none"}}}),
            "invalid_request",
        ),
    ] {
        let response = f
            .request(reqwest::Method::POST, url.clone(), &lobby.host)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        let error: serde_json::Value = response.json().await.unwrap();
        assert_eq!(error["code"], code);
    }
    let response = f
        .request(reqwest::Method::POST, url, &lobby.host)
        .json(&json!({"mode_id": "speed_round", "settings": {"rounds": 3}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["game"]["game"]["total_rounds"], 3);
    server.abort();
}
