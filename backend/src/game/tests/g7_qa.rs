//! G7: the rules decided after QA played full games of every mode. Ties and byes,
//! small lobbies, answer matching on curated titles, the board view and request
//! validation. Each test names the QA finding it covers.
use super::*;
use crate::game::{GameError, rules::*};
use reqwest::Method;
use serde_json::{Value, json};

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

async fn stop(f: &Fixture, lobby: &Lobby) -> GameView {
    f.database
        .stop_game(&lobby.host, &lobby.host_membership, || f.now())
        .await
        .unwrap()
        .game
        .game
        .unwrap()
}

async fn dismiss(f: &Fixture, lobby: &Lobby) {
    f.database
        .stop_game(&lobby.host, &lobby.host_membership, || f.now())
        .await
        .unwrap();
}

fn points(game: &GameView, user: &AuthenticatedUser) -> i64 {
    game.scoreboard
        .iter()
        .find(|s| s.user_id == user.id.to_string())
        .map_or(0, |s| s.points)
}

async fn raw(
    f: &Fixture,
    method: Method,
    url: String,
    user: &AuthenticatedUser,
    body: &str,
) -> (u16, Value) {
    let response = f
        .request(method, url, user)
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

// Duel tie game: nobody votes, so every pair ends 0-0.
#[tokio::test]
async fn duel_ties_play_the_earliest_song_but_award_no_points() {
    let f = Fixture::new().await;
    let lobby = f.lobby(3).await;
    f.start(&lobby, "duel").await;
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
    let (g0, m0) = &lobby.guests[0];
    assert_eq!(f.view(g0, m0).await.phase, Phase::Vote);
    f.advance(30);
    f.tick().await;
    let game = f.view(g0, m0).await;
    let result = game.last_result.as_ref().unwrap();
    assert!(
        result
            .entries
            .iter()
            .all(|e| e.outcome == ResultOutcome::Tie && e.points == 0)
    );
    let playing: Vec<_> = result
        .entries
        .iter()
        .filter(|e| e.winner)
        .map(|e| e.submission_id.clone())
        .collect();
    assert_eq!(
        playing,
        vec![ids[0].clone(), ids[2].clone()],
        "earliest of each pair"
    );
    assert!(game.scoreboard.iter().all(|s| s.points == 0));
    assert_eq!(
        game.now_playing.unwrap().song.unwrap().video_id,
        "hosthost01"
    );
}

// The 2-player Duel.
#[tokio::test]
async fn duel_needs_three_players() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    assert!(matches!(
        f.start_with(&lobby, "duel", json!({})).await,
        Err(GameError::NotEnoughPlayers(_))
    ));
    // Party runs with two.
    f.start(&lobby, "party").await;
    stop(&f, &lobby).await;

    let (base, server) = f.http().await;
    let (status, error) = raw(
        &f,
        Method::POST,
        format!(
            "{base}/api/lobbies/memberships/{}/game",
            lobby.host_membership
        ),
        &lobby.host,
        r#"{"mode_id":"duel"}"#,
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (409, Some("not_enough_players"))
    );
    assert!(error["error"].as_str().unwrap().contains("3 players"));
    server.abort();
}

// Duel with 2 songs each.
#[tokio::test]
async fn duel_with_two_songs_each_never_pairs_a_player_with_themselves() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    f.start_with(&lobby, "duel", json!({"songs_per_player": 2}))
        .await
        .unwrap();
    let mut players = vec![(lobby.host.clone(), lobby.host_membership.clone())];
    players.extend(lobby.guests.iter().cloned());
    // Each player submits both songs back to back, the order QA used.
    for (i, (user, membership)) in players.iter().enumerate() {
        for n in 0..2 {
            f.submit(user, membership, &format!("p{i}song{n:04}"))
                .await
                .unwrap();
        }
    }
    let (g0, m0) = &lobby.guests[0];
    let ballot = f.view(g0, m0).await.ballot.unwrap();
    assert_eq!(ballot.groups.len(), 3);
    let owner = |entry: &BallotEntry| entry.song.video_id[1..2].to_owned();
    for group in &ballot.groups {
        assert_eq!(group.entries.len(), 2);
        assert_ne!(owner(&group.entries[0]), owner(&group.entries[1]));
    }
    // g0 has a song in two pairs, so they vote in the one pair without theirs.
    assert_eq!(ballot.votes_allowed, 1);
}

// Voting for the other song in your own pair.
#[tokio::test]
async fn voting_in_your_own_pair_says_so() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    f.start(&lobby, "duel").await;
    let host_song = f
        .submit(&lobby.host, &lobby.host_membership, "hosthost01")
        .await
        .unwrap()
        .submission_id;
    let (g0, m0) = &lobby.guests[0];
    let mine = f.submit(g0, m0, "guestaaa01").await.unwrap().submission_id;
    let (g1, m1) = &lobby.guests[1];
    f.submit(g1, m1, "guestbbb01").await.unwrap();
    assert!(matches!(
        f.vote(g0, m0, &host_song).await,
        Err(GameError::OwnPair)
    ));
    assert!(matches!(
        f.vote(g0, m0, &mine).await,
        Err(GameError::SelfVote)
    ));
    let (base, server) = f.http().await;
    let (status, error) = raw(
        &f,
        Method::POST,
        format!("{base}/api/lobbies/memberships/{m0}/game/votes"),
        g0,
        &json!({"submission_id": host_song}).to_string(),
    )
    .await;
    assert_eq!((status, error["code"].as_str()), (409, Some("own_pair")));
    server.abort();
}

// Party duplicate pick.
#[tokio::test]
async fn the_same_video_can_be_picked_once_per_round() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    f.start(&lobby, "party").await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    f.submit(g0, m0, "samesong011").await.unwrap();
    assert!(matches!(
        f.submit(g1, m1, "samesong011").await,
        Err(GameError::SongAlreadyPicked)
    ));
    // The refusal costs nothing: g1 picks something else.
    f.submit(g1, m1, "othersong1").await.unwrap();
    // A retry of the accepted pick is still idempotent, not a duplicate.
    assert!(!f.submit(g0, m0, "samesong011").await.unwrap().changed);

    let (base, server) = f.http().await;
    f.mock_metadata("Artist - Same (Official Video)", "Artist")
        .await;
    let url = |id: &str| format!("https://www.youtube.com/watch?v={id}");
    let submit = format!(
        "{base}/api/lobbies/memberships/{}/game/submissions",
        lobby.host_membership
    );
    let (status, error) = raw(
        &f,
        Method::POST,
        submit,
        &lobby.host,
        &json!({"url": url("samesong011"), "request_id": "a".repeat(32)}).to_string(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (409, Some("song_already_picked"))
    );
    assert_eq!(
        error["error"],
        "Someone already picked that song this round. Pick another."
    );
    server.abort();
}

// Party with more than one song per player shows points per song.
#[tokio::test]
async fn party_results_show_points_per_song() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    f.start_with(
        &lobby,
        "party",
        json!({"songs_per_player": 2, "votes_per_player": 2}),
    )
    .await
    .unwrap();
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    let a = f.submit(g0, m0, "g0songone1").await.unwrap().submission_id;
    let b = f.submit(g0, m0, "g0songtwo1").await.unwrap().submission_id;
    let c = f.submit(g1, m1, "g1songone1").await.unwrap().submission_id;
    f.submit(g1, m1, "g1songtwo1").await.unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hostsong01")
        .await
        .unwrap();
    f.submit(&lobby.host, &lobby.host_membership, "hostsong02")
        .await
        .unwrap();
    f.vote(g1, m1, &a).await.unwrap();
    f.vote(g1, m1, &b).await.unwrap();
    f.vote(&lobby.host, &lobby.host_membership, &a)
        .await
        .unwrap();
    f.vote(&lobby.host, &lobby.host_membership, &c)
        .await
        .unwrap();
    f.advance(30);
    f.tick().await;
    let game = f.view(g0, m0).await;
    let result = game.last_result.as_ref().unwrap();
    let earned = |id: &str| {
        result
            .entries
            .iter()
            .find(|e| e.submission_id == id)
            .unwrap()
            .points
    };
    assert_eq!(earned(&a), 2 * POINTS_PER_VOTE);
    assert_eq!(earned(&b), POINTS_PER_VOTE);
    assert_eq!(earned(&c), POINTS_PER_VOTE);
    assert_eq!(
        points(&game, g0),
        3 * POINTS_PER_VOTE,
        "the scoreboard adds them up"
    );
}

// Name That Tune with bracketed titles.
#[tokio::test]
async fn curated_bracketed_titles_accept_the_song_name_alone() {
    let f = Fixture::new().await;
    f.database
        .import_starters(
            r#"[{"video_id":"0UPDBODtxzw","title":"Dracula (JENNIE Remix)","artist":"Tame Impala, JENNIE","thumbnail_url":"https://i.ytimg.com/vi/0UPDBODtxzw/hqdefault.jpg"},
                {"video_id":"w2Ov5jzm3j8","title":"Old Town Road (Remix)","artist":"Lil Nas X, Billy Ray Cyrus","thumbnail_url":"https://i.ytimg.com/vi/w2Ov5jzm3j8/hqdefault.jpg"},
                {"video_id":"t7bQwwqW-Hc","title":"A Bar Song (Tipsy)","artist":"Shaboozey","thumbnail_url":"https://i.ytimg.com/vi/t7bQwwqW-Hc/hqdefault.jpg"}]"#,
            NOW,
        )
        .await
        .unwrap();
    let lobby = f.lobby(3).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    let (g2, m2) = &lobby.guests[2];
    f.start_with(&lobby, "name_that_tune", json!({"songs_per_player": 3}))
        .await
        .unwrap();
    for (video, title) in [
        (
            "0UPDBODtxzw",
            "Tame Impala, JENNIE - Dracula (JENNIE Remix) (Official Visualizer)",
        ),
        (
            "w2Ov5jzm3j8",
            "Lil Nas X - Old Town Road (Official Video) ft. Billy Ray Cyrus",
        ),
        (
            "t7bQwwqW-Hc",
            "Shaboozey - A Bar Song (Tipsy) [Official Visualizer]",
        ),
    ] {
        propose(&f, g0, m0, video, title).await.unwrap();
    }
    f.advance(90);
    f.tick().await;
    for (round, (short, full)) in [
        ("Dracula", "dracula jennie remix"),
        ("Old Town Road", "Old Town Road (Remix)"),
        ("A Bar Song", "a bar song tipsy"),
    ]
    .into_iter()
    .enumerate()
    {
        let game = f.view(g1, m1).await;
        assert_eq!((game.round, game.phase), (round as i64 + 1, Phase::Guess));
        assert!(guess(&f, g1, m1, short).await.unwrap().correct, "{short}");
        assert!(guess(&f, g2, m2, full).await.unwrap().correct, "{full}");
        let result = f.view(g1, m1).await.last_result.unwrap();
        assert_eq!(result.guesses.len(), 2);
        f.advance(8);
        f.tick().await;
    }
    assert_eq!(f.view(g1, m1).await.status, GameStatus::Ended);
}

// A 2-player Name That Tune: the guest's own song has nobody to guess it.
#[tokio::test]
async fn rounds_nobody_can_guess_are_skipped_with_a_short_reveal() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (g0, m0) = &lobby.guests[0];
    f.start_with(&lobby, "name_that_tune", json!({"songs_per_player": 1}))
        .await
        .unwrap();
    propose(&f, g0, m0, "guestsong1", "Adele - Hello")
        .await
        .unwrap();
    propose(
        &f,
        &lobby.host,
        &lobby.host_membership,
        "hostsong01",
        "Toto - Africa",
    )
    .await
    .unwrap();
    // Everyone proposed, so collect closes now: round 1 is g0's song.
    let game = f.view(g0, m0).await;
    assert_eq!((game.round, game.phase), (1, Phase::Reveal));
    let result = game.last_result.unwrap();
    assert_eq!(result.note, Some(ResultNote::NobodyToGuess));
    assert_eq!(result.answer.as_deref(), Some("Hello"));
    assert!(game.now_playing.is_none(), "a skipped round plays nothing");
    assert_eq!(game.phase_ends_at.unwrap().timestamp(), f.now() + 3);
    // Round 2 is the host's song, which g0 can guess.
    f.advance(3);
    f.tick().await;
    let game = f.view(g0, m0).await;
    assert_eq!((game.round, game.phase), (2, Phase::Guess));
    assert!(game.guess.unwrap().eligible);
}

// Ending mid-guess and a game with no proposals.
#[tokio::test]
async fn ending_mid_guess_reveals_the_song_and_no_proposals_says_so() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    f.start_with(&lobby, "name_that_tune", json!({"songs_per_player": 1}))
        .await
        .unwrap();
    propose(&f, g0, m0, "guestsong1", "Adele - Hello")
        .await
        .unwrap();
    f.advance(90);
    f.tick().await;
    assert!(guess(&f, g1, m1, "hello").await.unwrap().correct);
    assert_eq!(
        f.view(g1, m1).await.phase,
        Phase::Reveal,
        "only guesser done"
    );

    // Again, but end while the clip is still being guessed.
    stop(&f, &lobby).await;
    dismiss(&f, &lobby).await;
    f.start_with(&lobby, "name_that_tune", json!({"songs_per_player": 1}))
        .await
        .unwrap();
    propose(&f, g0, m0, "guestsong2", "Toto - Africa")
        .await
        .unwrap();
    f.advance(90);
    f.tick().await;
    assert_eq!(f.view(g1, m1).await.phase, Phase::Guess);
    let ended = stop(&f, &lobby).await;
    assert_eq!(ended.status, GameStatus::Ended);
    let result = ended.last_result.unwrap();
    assert_eq!(result.answer.as_deref(), Some("Africa"));
    assert_eq!(result.note, Some(ResultNote::EndedEarly));
    assert_eq!(
        f.view(g1, m1).await.last_result.unwrap().answer.as_deref(),
        Some("Africa")
    );

    dismiss(&f, &lobby).await;
    f.start(&lobby, "name_that_tune").await;
    f.advance(90);
    f.tick().await;
    let game = f.view(g1, m1).await;
    assert_eq!(game.status, GameStatus::Ended);
    assert_eq!(
        game.last_result.unwrap().note,
        Some(ResultNote::NoProposals)
    );
}

// The board's network responses never carry the answer, even on the host's account.
#[tokio::test]
async fn the_board_view_never_receives_the_answer() {
    let f = Fixture::new().await;
    let lobby = f.lobby(2).await;
    let (g0, m0) = &lobby.guests[0];
    f.start_with(&lobby, "name_that_tune", json!({"songs_per_player": 1}))
        .await
        .unwrap();
    propose(
        &f,
        &lobby.host,
        &lobby.host_membership,
        "hostsecret",
        "Queen - Bohemian Rhapsody",
    )
    .await
    .unwrap();
    propose(&f, g0, m0, "guestsong1", "Adele - Hello")
        .await
        .unwrap();
    let (base, server) = f.http().await;
    let game_url = format!(
        "{base}/api/lobbies/memberships/{}/game",
        lobby.host_membership
    );
    // Collect: the host's own secret proposal is not on the board.
    let (status, board) = raw(
        &f,
        Method::GET,
        format!("{game_url}?view=board"),
        &lobby.host,
        "",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(board["game"]["my_submissions"], json!([]));
    f.advance(90);
    f.tick().await;
    let (_, host) = raw(&f, Method::GET, game_url.clone(), &lobby.host, "").await;
    assert_eq!(host["game"]["phase"], "guess");
    assert!(
        host["game"]["now_playing"]["song"].is_object(),
        "the host player needs the video"
    );
    let (status, board) = raw(
        &f,
        Method::GET,
        format!("{game_url}?view=board"),
        &lobby.host,
        "",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(board["game"]["now_playing"]["hidden"], true);
    assert!(board["game"]["now_playing"]["song"].is_null());
    let text = board.to_string();
    for secret in ["Bohemian", "hostsecret", "Hello", "guestsong1"] {
        assert!(!text.contains(secret), "board leaked {secret}: {text}");
    }
    let (status, _) = raw(
        &f,
        Method::GET,
        format!("{game_url}?view=tv"),
        &lobby.host,
        "",
    )
    .await;
    assert_eq!(status, 400, "unknown views are refused, not ignored");
    server.abort();
}

// Duplicate keys inside settings.
#[tokio::test]
async fn a_duplicate_settings_key_is_rejected() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (base, server) = f.http().await;
    let start = format!(
        "{base}/api/lobbies/memberships/{}/game",
        lobby.host_membership
    );
    let (status, error) = raw(
        &f,
        Method::POST,
        start.clone(),
        &lobby.host,
        r#"{"mode_id":"party","settings":{"rounds":3,"rounds":50}}"#,
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (400, Some("invalid_request"))
    );
    let (status, _) = raw(
        &f,
        Method::POST,
        format!("{base}/api/game-modes"),
        &lobby.host,
        r#"{"name":"x","rules":{"prompt":"none","collect":"pick","ballot":"all","winner":"most_votes","challenge":"listen","scoring":"votes_to_picker"},"settings":{"rounds":3,"rounds":3}}"#,
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = raw(
        &f,
        Method::POST,
        start,
        &lobby.host,
        r#"{"mode_id":"party","settings":{"rounds":3}}"#,
    )
    .await;
    assert_eq!(status, 200, "unique keys still start");
    server.abort();
}

// My modes: duplicate names become unique.
#[tokio::test]
async fn duplicate_mode_names_get_a_number() {
    let f = Fixture::new().await;
    let lobby = f.lobby(0).await;
    let rules: Rules = serde_json::from_value(json!({"prompt":"none","collect":"pick","ballot":"all","winner":"most_votes","challenge":"listen","scoring":"votes_to_picker"})).unwrap();
    let save = |name: &str| {
        let definition = crate::game::custom_definition(name, rules).unwrap();
        let f = &f;
        let host = lobby.host.clone();
        async move {
            f.database
                .save_game_mode(&host, &definition, &[], || f.now())
                .await
                .unwrap()
                .definition
                .name
        }
    };
    assert_eq!(save("Friday").await, "Friday");
    assert_eq!(
        save("friday").await,
        "friday 2",
        "names compare ignoring case"
    );
    assert_eq!(save("Friday").await, "Friday 3");
    assert_eq!(save("Party").await, "Party 2", "preset names are taken too");
    let long = "x".repeat(40);
    assert_eq!(save(&long).await, long);
    let second = save(&long).await;
    assert_eq!(second, format!("{} 2", "x".repeat(38)));
    assert_eq!(second.chars().count(), 40);
    // Emoji count as one character each, like Array.from in the browser.
    let notes = "\u{1F3B5}".repeat(40);
    assert_eq!(crate::game::mode_name(&notes).unwrap(), notes);
    assert!(crate::game::mode_name(&"\u{1F3B5}".repeat(41)).is_err());
    for hidden in [
        "a\u{202E}b",
        "a\u{200B}b",
        "a\u{2066}b",
        "\u{FEFF}a",
        "a\u{200D}b",
    ] {
        assert!(matches!(
            crate::game::mode_name(hidden),
            Err(GameError::InvalidModeName(_))
        ));
    }
}

// Mode names follow the nickname character rules, with 40 characters.
#[tokio::test]
async fn mode_names_share_the_nickname_character_rules() {
    use crate::game::mode_name;
    for bad in [
        "a\u{0000}b",
        "a\nb",
        "a\tb",
        "a\u{2028}b",
        "a\u{2029}b",
        "a\u{00AD}b",
        "a\u{0085}b",
        "a\u{200C}b",
        "a\u{2060}b",
        "a\u{202A}b",
        "Kat\u{FE0F}",
        "Kat\u{200D}",
    ] {
        assert!(
            matches!(mode_name(bad), Err(GameError::InvalidModeName(_))),
            "{bad:?} should be refused"
        );
    }
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
    let flag = "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}";
    for good in [
        "Kat\u{2764}\u{FE0F}",
        family,
        flag,
        "1\u{FE0F}\u{20E3} round",
    ] {
        assert_eq!(mode_name(good).unwrap(), good);
    }
    assert_eq!(
        mode_name(&family.repeat(40)).unwrap(),
        family.repeat(40),
        "a ZWJ family is one character"
    );
    assert!(mode_name(&family.repeat(41)).is_err());
    let padded = format!("  {}  ", "B".repeat(40));
    assert_eq!(mode_name(&padded).unwrap(), "B".repeat(40), "trimmed first");
    assert_eq!(mode_name("  Rainy   Duel ").unwrap(), "Rainy Duel");
    // Nicknames use the very same rules.
    assert_eq!(crate::database::nicknames::normalize("a\u{2028}b"), None);

    // A clash keeps whole emoji when it shortens the name for the suffix.
    let f = Fixture::new().await;
    let lobby = f.lobby(0).await;
    let rules: Rules = serde_json::from_value(json!({"prompt":"none","collect":"pick","ballot":"all","winner":"most_votes","challenge":"listen","scoring":"votes_to_picker"})).unwrap();
    let definition = crate::game::custom_definition(&family.repeat(40), rules).unwrap();
    let mut names = Vec::new();
    for _ in 0..2 {
        names.push(
            f.database
                .save_game_mode(&lobby.host, &definition, &[], || f.now())
                .await
                .unwrap()
                .definition
                .name,
        );
    }
    assert_eq!(names[1], format!("{} 2", family.repeat(38)));
}
