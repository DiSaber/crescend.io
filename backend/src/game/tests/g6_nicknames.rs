//! Nicknames in game snapshots: scoreboard, results, now playing and tune reveals.
use super::*;

impl Fixture {
    /// Host "Daniel" and guests with the given names, joined through the named paths.
    async fn named_lobby(&self, names: &[&str]) -> Lobby {
        let host = self.account("host").await;
        let view = self
            .database
            .create_named_lobby(host.id, Some("Daniel"), || self.now())
            .await
            .unwrap();
        let code = view.lobby.unwrap().join_code;
        let mut guests = Vec::new();
        for name in names {
            let user = self.account("guest").await;
            let (view, _, _) = self
                .database
                .join_lobby_named(
                    user.id,
                    &code,
                    Some(name),
                    || self.now(),
                    || {
                        crate::models::lobby::MembershipId::random()
                            .map_err(|_| crate::database::lobbies::LobbyError::Unavailable)
                    },
                )
                .await
                .unwrap();
            guests.push((user, view.membership_id.unwrap().as_str().to_owned()));
        }
        Lobby {
            host,
            host_membership: view.membership_id.unwrap().as_str().into(),
            guests,
        }
    }
}

#[tokio::test]
async fn party_results_and_scoreboard_carry_nicknames_even_after_leaving() {
    let f = Fixture::new().await;
    let lobby = f.named_lobby(&["Kat", "<svg onload=alert(1)>"]).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    f.start(&lobby, "party").await;
    let a = f.submit(g0, m0, "dQw4w9WgXcQ").await.unwrap().submission_id;
    f.submit(g1, m1, "fJ9rUzIMcZQ").await.unwrap();
    let seq = f.view(&lobby.host, &lobby.host_membership).await.phase_seq;
    f.database
        .advance_game(
            &lobby.host,
            &lobby.host_membership,
            &GameAdvanceRequest {
                phase_seq: Some(seq),
                play_seq: None,
            },
            || f.now(),
        )
        .await
        .unwrap();
    let ballot = f.view(g1, m1).await.ballot.unwrap();
    // Ballots stay anonymous: no names or IDs of submitters.
    let json = serde_json::to_string(&ballot).unwrap();
    assert!(!json.contains("Kat") && !json.contains("user_id"), "{json}");
    f.vote(g1, m1, &a).await.unwrap();
    f.vote(&lobby.host, &lobby.host_membership, &a).await.ok();
    let seq = f.view(&lobby.host, &lobby.host_membership).await.phase_seq;
    f.database
        .advance_game(
            &lobby.host,
            &lobby.host_membership,
            &GameAdvanceRequest {
                phase_seq: Some(seq),
                play_seq: None,
            },
            || f.now(),
        )
        .await
        .unwrap();
    let view = f.view(g0, m0).await;
    let result = view.last_result.as_ref().unwrap();
    let by_user = |id: i64| {
        result
            .entries
            .iter()
            .find(|e| e.submitted_by_user_id == id.to_string())
            .unwrap()
            .submitted_by_nickname
            .clone()
    };
    assert_eq!(by_user(g0.id), "Kat");
    assert_eq!(by_user(g1.id), "<svg onload=alert(1)>");
    let playing = view.now_playing.as_ref().unwrap();
    assert_eq!(playing.submitted_by_nickname.as_deref(), Some("Kat"));
    let names: Vec<_> = view
        .scoreboard
        .iter()
        .map(|s| s.nickname.as_str())
        .collect();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"Daniel") && names.contains(&"Kat"));

    // A departed player keeps their name on the scoreboard (marked inactive).
    f.database.leave_lobby(g0.id, m0, || f.now()).await.unwrap();
    let view = f.view(g1, m1).await;
    let kat = view
        .scoreboard
        .iter()
        .find(|s| s.user_id == g0.id.to_string())
        .unwrap();
    assert_eq!((kat.nickname.as_str(), kat.active), ("Kat", false));
    assert_eq!(
        view.last_result.unwrap().entries[0].submitted_by_nickname,
        "Kat",
        "stored results are named on every read"
    );
}

#[tokio::test]
async fn tune_reveal_names_guessers_and_proposer_but_not_while_hidden() {
    let f = Fixture::new().await;
    let lobby = f.named_lobby(&["Kat", "Bo"]).await;
    let (g0, m0) = &lobby.guests[0];
    let (g1, m1) = &lobby.guests[1];
    f.start(&lobby, "name_that_tune").await;
    let mut song = song("fJ9rUzIMcZQ");
    song.title = "Queen - Bohemian Rhapsody (Official Video Remastered)".into();
    f.database
        .submit_game(g0, m0, &request_id(g0, "fJ9rUzIMcZQ"), &song, || f.now())
        .await
        .unwrap();
    f.advance(90);
    f.tick().await;
    let hidden = f.view(g1, m1).await.now_playing.unwrap();
    assert!(hidden.hidden && hidden.submitted_by_nickname.is_none());
    let host = f
        .view(&lobby.host, &lobby.host_membership)
        .await
        .now_playing
        .unwrap();
    assert_eq!(host.submitted_by_nickname.as_deref(), Some("Kat"));
    let guessed = f
        .database
        .guess_game(g1, m1, "bohemian rhapsody", || f.now())
        .await
        .unwrap();
    let game = guessed.game.game.unwrap();
    assert_eq!(game.phase, Phase::Reveal);
    let result = game.last_result.unwrap();
    assert_eq!(result.entries[0].submitted_by_nickname, "Kat");
    assert_eq!(result.guesses[0].nickname, "Bo");
    let bo = game
        .scoreboard
        .iter()
        .find(|s| s.user_id == g1.id.to_string())
        .unwrap();
    assert_eq!(bo.nickname, "Bo");
}
