use super::*;
use crate::game::{GameError, rules::*};
use serde_json::json;

async fn propose(
    f: &Fixture,
    user: &AuthenticatedUser,
    membership: &str,
    video: &str,
    title: &str,
) -> Result<GameSubmissionResponse, GameError> {
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
}

async fn guess(
    f: &Fixture,
    user: &AuthenticatedUser,
    membership: &str,
    text: &str,
) -> Result<GameGuessResponse, GameError> {
    f.database
        .guess_game(user, membership, text, || f.now())
        .await
}

async fn skip(f: &Fixture, lobby: &Lobby) -> GameView {
    let seq = f.view(&lobby.host, &lobby.host_membership).await.phase_seq;
    let request = GameAdvanceRequest {
        phase_seq: Some(seq),
        play_seq: None,
    };
    f.database
        .advance_game(&lobby.host, &lobby.host_membership, &request, || f.now())
        .await
        .unwrap()
        .game
        .game
        .unwrap()
}

#[tokio::test]
async fn name_that_tune_hides_proposals_and_titles_then_scores_speed_and_proposers() {
    let f = Fixture::new().await;
    let lobby = f.lobby(3).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    let (g2, m2) = &lobby.guests[2];
    let game = f.start(&lobby, "name_that_tune").await.game.unwrap();
    assert_eq!((game.phase, game.total_rounds), (Phase::Collect, None));
    assert_eq!(game.phase_ends_at.unwrap().timestamp(), NOW + 90);

    let queen = "Queen - Bohemian Rhapsody (Official Video Remastered)";
    propose(&f, g0, m0, "fJ9rUzIMcZQ", queen).await.unwrap();
    propose(
        &f,
        g1,
        m1,
        "hTWKbfoikeg",
        "Nirvana - Smells Like Teen Spirit (Official Music Video)",
    )
    .await
    .unwrap();
    propose(
        &f,
        g0,
        m0,
        "kJQP7kiw5Fk",
        "Luis Fonsi - Despacito ft. Daddy Yankee",
    )
    .await
    .unwrap();
    assert!(matches!(
        propose(
            &f,
            g0,
            m0,
            "dQw4w9WgXcQ",
            "Rick Astley - Never Gonna Give You Up"
        )
        .await,
        Err(GameError::SubmissionLimit)
    ));
    assert!(matches!(
        propose(&f, g2, m2, "fJ9rUzIMcZQ", queen).await,
        Err(GameError::DuplicateSong)
    ));
    propose(
        &f,
        &lobby.host,
        &lobby.host_membership,
        "1w7OgIMMRc4",
        "Guns N' Roses - Sweet Child O' Mine (Official Music Video)",
    )
    .await
    .unwrap();
    // Proposals are secret: each player sees only their own.
    let seen = f.view(g1, m1).await;
    assert_eq!(seen.my_submissions.len(), 1);
    assert_eq!(seen.submission_count, 4);
    let json = serde_json::to_string(&seen).unwrap();
    assert!(!json.contains("Bohemian") && !json.contains("Despacito"));

    f.advance(90);
    f.tick().await;
    // Round robin by first proposal: g0, g1, host, then g0's second song.
    let host_view = f.view(&lobby.host, &lobby.host_membership).await;
    assert_eq!((host_view.round, host_view.phase), (1, Phase::Guess));
    assert_eq!(host_view.total_rounds, Some(4));
    assert_eq!(host_view.phase_ends_at.unwrap().timestamp(), f.now() + 30);
    let host_playing = host_view.now_playing.unwrap();
    assert!(!host_playing.hidden);
    assert_eq!(host_playing.song.as_ref().unwrap().video_id, "fJ9rUzIMcZQ");
    assert_eq!(host_playing.start_seconds, 30);
    assert!(!host_view.guess.unwrap().eligible);

    let guesser = f.view(g1, m1).await;
    let hidden = guesser.now_playing.as_ref().unwrap();
    assert!(hidden.hidden && hidden.song.is_none() && hidden.submitted_by_user_id.is_none());
    let mine = guesser.guess.as_ref().unwrap();
    assert!(mine.eligible && !mine.own_song && !mine.correct);
    assert_eq!(mine.guesses_left, crate::game::answer::MAX_WRONG_GUESSES);
    let json = serde_json::to_string(&guesser).unwrap();
    assert!(
        !json.contains("Bohemian") && !json.contains("fJ9rUzIMcZQ"),
        "{json}"
    );
    let proposer = f.view(g0, m0).await.guess.unwrap();
    assert!(proposer.own_song && !proposer.eligible);

    assert!(matches!(
        guess(&f, g0, m0, "bohemian rhapsody").await,
        Err(GameError::OwnSong)
    ));
    assert!(matches!(
        guess(&f, &lobby.host, &lobby.host_membership, "bohemian rhapsody").await,
        Err(GameError::HostCannotGuess)
    ));
    assert!(matches!(
        f.vote(g1, m1, &"a".repeat(32)).await,
        Err(GameError::PhaseClosed)
    ));
    let wrong = guess(&f, g1, m1, "queen").await.unwrap();
    assert!(wrong.changed && !wrong.correct);
    assert_eq!(wrong.game.game.unwrap().guess.unwrap().wrong_guesses, 1);

    f.advance(10);
    let first = guess(&f, g1, m1, "bohemian rapsody").await.unwrap();
    assert!(first.correct);
    let expected = GUESS_BASE + (GUESS_SPEED * 20 + 15) / 30 + FIRST_GUESS_BONUS;
    assert_eq!(first.points, expected);
    let repeat = guess(&f, g1, m1, "anything").await.unwrap();
    assert!(!repeat.changed && repeat.correct && repeat.points == expected);
    assert_eq!(f.view(g2, m2).await.guess.unwrap().correct_count, 1);

    f.advance(5);
    let second = guess(&f, g2, m2, "Bohemian Rhapsody").await.unwrap();
    assert_eq!(second.points, GUESS_BASE + GUESS_SPEED / 2);
    // Every eligible guesser is right: the reveal starts early and shows everything.
    let game = second.game.game.unwrap();
    assert_eq!(game.phase, Phase::Reveal);
    let result = game.last_result.unwrap();
    assert_eq!(result.answer.as_deref(), Some("Bohemian Rhapsody"));
    assert_eq!(result.entries[0].submitted_by_user_id, g0.id.to_string());
    assert_eq!(result.entries[0].points, 2 * PROPOSER_PER_GUESS);
    let ranks: Vec<_> = result
        .guesses
        .iter()
        .map(|g| (g.user_id.clone(), g.rank))
        .collect();
    assert_eq!(ranks, vec![(g1.id.to_string(), 1), (g2.id.to_string(), 2)]);
    let revealed = game.now_playing.unwrap();
    assert!(
        !revealed.hidden && revealed.song.is_some(),
        "the clip plays on during the reveal"
    );

    f.advance(8);
    f.tick().await;
    let game = f.view(g1, m1).await;
    assert_eq!((game.round, game.phase), (2, Phase::Guess));
    assert!(game.guess.unwrap().own_song, "round 2 plays g1's song");
    assert_eq!(
        game.up_next_count, 0,
        "the finished clip left the play list"
    );

    // Nobody guesses round 2; the timer reveals it and its proposer scores nothing.
    f.advance(30);
    f.tick().await;
    let game = f.view(g1, m1).await;
    assert_eq!(game.phase, Phase::Reveal);
    assert!(game.last_result.unwrap().guesses.is_empty());
    skip(&f, &lobby).await;
    skip(&f, &lobby).await;
    skip(&f, &lobby).await;
    let game = skip(&f, &lobby).await;
    assert_eq!((game.round, game.phase), (4, Phase::Reveal));
    let game = skip(&f, &lobby).await;
    assert_eq!(game.status, GameStatus::Ended);
    let scores: Vec<_> = game
        .scoreboard
        .iter()
        .map(|s| (s.user_id.clone(), s.points))
        .collect();
    assert_eq!(scores[0], (g1.id.to_string(), expected));
    assert_eq!(scores[1], (g2.id.to_string(), GUESS_BASE + GUESS_SPEED / 2));
    assert_eq!(scores[2], (g0.id.to_string(), 2 * PROPOSER_PER_GUESS));
}

#[tokio::test]
async fn starter_titles_are_the_answer_key_when_the_song_has_one() {
    let f = Fixture::new().await;
    f.database
        .import_starters(
            r#"[{"video_id":"hLOheGDwD_0","title":"Choosin' Texas","artist":"Ella Langley","thumbnail_url":"https://i.ytimg.com/vi/hLOheGDwD_0/hqdefault.jpg"}]"#,
            NOW,
        )
        .await
        .unwrap();
    let lobby = f.lobby(2).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    f.start(&lobby, "name_that_tune").await;
    propose(
        &f,
        g0,
        m0,
        "hLOheGDwD_0",
        "Ella Langley - Choosin&#39; Texas (Official Lyric Video) ft. Nobody",
    )
    .await
    .unwrap();
    f.advance(90);
    f.tick().await;
    assert!(guess(&f, g1, m1, "choosin texas").await.unwrap().correct);
    let result = f.view(g1, m1).await.last_result.unwrap();
    assert_eq!(result.answer.as_deref(), Some("Choosin' Texas"));
}

#[tokio::test]
async fn guesses_are_limited_and_the_round_count_follows_settings_and_proposals() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    assert!(matches!(
        f.start_with(&lobby, "name_that_tune", json!({"play_seconds": 5}))
            .await,
        Err(GameError::InvalidSettings)
    ));
    f.start_with(
        &lobby,
        "name_that_tune",
        json!({"rounds": 2, "songs_per_player": 3}),
    )
    .await
    .unwrap();
    for (i, title) in [
        "Adele - Hello",
        "Toto - Africa (Official HD Video)",
        "Beyoncé - Halo",
    ]
    .iter()
    .enumerate()
    {
        propose(&f, g0, m0, &format!("video{i:05}"), title)
            .await
            .unwrap();
    }
    f.advance(90);
    f.tick().await;
    let game = f.view(g1, m1).await;
    assert_eq!(game.total_rounds, Some(2), "rounds caps the proposals");
    for _ in 0..crate::game::answer::MAX_WRONG_GUESSES {
        assert!(!guess(&f, g1, m1, "nope").await.unwrap().correct);
    }
    assert!(matches!(
        guess(&f, g1, m1, "hello").await,
        Err(GameError::GuessLimit)
    ));
    assert_eq!(f.view(g1, m1).await.guess.unwrap().guesses_left, 0);
    skip(&f, &lobby).await;
    let game = skip(&f, &lobby).await;
    assert_eq!(game.round, 2);
    assert!(
        guess(&f, g1, m1, "africa").await.unwrap().correct,
        "limits reset each round"
    );

    // A game nobody proposed to ends when collect closes.
    f.database
        .stop_game(&lobby.host, &lobby.host_membership, || f.now())
        .await
        .unwrap();
    f.start(&lobby, "name_that_tune").await;
    f.advance(90);
    f.tick().await;
    assert_eq!(f.view(g1, m1).await.status, GameStatus::Ended);
}

#[tokio::test]
async fn http_guesses_validate_text_and_never_leak_the_answer() {
    let f = Fixture::new().await;
    f.mock_metadata(
        "Nirvana - Smells Like Teen Spirit (Official Music Video)",
        "Nirvana",
    )
    .await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    let (base, server) = f.http().await;
    let url = |m: &str, suffix: &str| format!("{base}/api/lobbies/memberships/{m}/game{suffix}");
    let response = f
        .request(
            reqwest::Method::POST,
            url(&lobby.host_membership, ""),
            &lobby.host,
        )
        .json(&json!({"mode_id": "name_that_tune"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = f
        .request(
            reqwest::Method::POST,
            url(&lobby.host_membership, "/submissions"),
            &lobby.host,
        )
        .json(&json!({"url": "https://youtu.be/hTWKbfoikeg", "request_id": "3".repeat(32)}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    f.advance(90);
    f.tick().await;
    let raw = f
        .request(reqwest::Method::GET, url(gm, ""), guest)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(raw.contains("\"phase\":\"guess\""));
    assert!(
        !raw.contains("Spirit") && !raw.contains("hTWKbfoikeg") && !raw.contains("Nirvana"),
        "{raw}"
    );
    for text in ["", "   ", &"x".repeat(101)] {
        let response = f
            .request(reqwest::Method::POST, url(gm, "/guesses"), guest)
            .json(&json!({"text": text}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }
    let response = f
        .request(
            reqwest::Method::POST,
            url(&lobby.host_membership, "/guesses"),
            &lobby.host,
        )
        .json(&json!({"text": "smells like teen spirit"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "host_cannot_guess");
    let mut hints = f.updates.subscribe();
    let body: serde_json::Value = f
        .request(reqwest::Method::POST, url(gm, "/guesses"), guest)
        .json(&json!({"text": "  Smells Like Teen Spirit  "}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["correct"], true);
    assert_eq!(body["game"]["game"]["phase"], "reveal");
    assert_eq!(
        body["game"]["game"]["last_result"]["answer"],
        "Smells Like Teen Spirit"
    );
    let revision: i64 = body["game"]["revision"].as_str().unwrap().parse().unwrap();
    assert_eq!(hints.recv().await.unwrap().1, revision);
    server.abort();
}
