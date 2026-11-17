//! G6 load: twenty players in one game over HTTP, every request in flight at once.
use super::*;
use futures_util::future::join_all;
use reqwest::Method;
use serde_json::{Value, json};
use std::time::Instant;

const PLAYERS: usize = 20;

async fn ok(response: reqwest::Response) -> Value {
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{body}");
    body
}

fn revision(body: &Value) -> i64 {
    body["game"]["revision"].as_str().unwrap().parse().unwrap()
}

/// Host first, then the 19 guests: (user, membership).
fn players(lobby: &Lobby) -> Vec<(&AuthenticatedUser, &str)> {
    std::iter::once((&lobby.host, lobby.host_membership.as_str()))
        .chain(lobby.guests.iter().map(|(u, m)| (u, m.as_str())))
        .collect()
}

#[tokio::test]
async fn twenty_players_pick_read_and_vote_at_once() {
    let f = Fixture::new().await;
    f.mock_metadata("Artist - Load Song (Official Video)", "Artist")
        .await;
    let lobby = f.lobby(PLAYERS - 1).await;
    let everyone = players(&lobby);
    let (base, server) = f.http().await;
    let url = |m: &str, suffix: &str| format!("{base}/api/lobbies/memberships/{m}/game{suffix}");
    let started = ok(f
        .request(Method::POST, url(&lobby.host_membership, ""), &lobby.host)
        .json(&json!({"mode_id": "party"}))
        .send()
        .await
        .unwrap())
    .await;
    let start_revision = revision(&started);

    // All twenty picks at once. Each commit bumps the revision exactly once, and the
    // last one in closes collect early.
    let clock = Instant::now();
    let picks = join_all(everyone.iter().enumerate().map(|(i, (user, m))| {
        f.request(Method::POST, url(m, "/submissions"), user)
            .json(&json!({"url": format!("https://youtu.be/load{i:07}"), "request_id": format!("{:032x}", i + 1)}))
            .send()
    }))
    .await;
    let picked = clock.elapsed();
    let mut revisions = Vec::new();
    let mut ids = Vec::new();
    for response in picks {
        let body = ok(response.unwrap()).await;
        revisions.push(revision(&body));
        ids.push(body["submission_id"].as_str().unwrap().to_owned());
    }
    revisions.sort();
    assert_eq!(
        revisions,
        (start_revision + 1..=start_revision + PLAYERS as i64).collect::<Vec<_>>(),
        "twenty serialized commits, no lost or doubled bumps"
    );

    // Twenty concurrent reads all see the same full ballot and their own entry.
    let clock = Instant::now();
    let reads = join_all(
        everyone
            .iter()
            .map(|(user, m)| f.request(Method::GET, url(m, ""), user).send()),
    )
    .await;
    let read = clock.elapsed();
    for (i, response) in reads.into_iter().enumerate() {
        let body = ok(response.unwrap()).await;
        let game = &body["game"];
        assert_eq!(game["phase"], "vote");
        let entries = game["ballot"]["groups"][0]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), PLAYERS);
        let mine: Vec<_> = entries.iter().filter(|e| e["mine"] == true).collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0]["submission_id"], ids[i].as_str());
        assert_eq!(game["scoreboard"].as_array().unwrap().len(), PLAYERS);
    }

    // Everyone votes at once for the next player's song; the last vote ends the round.
    let clock = Instant::now();
    let votes = join_all(everyone.iter().enumerate().map(|(i, (user, m))| {
        f.request(Method::POST, url(m, "/votes"), user)
            .json(&json!({"submission_id": ids[(i + 1) % PLAYERS]}))
            .send()
    }))
    .await;
    let voted = clock.elapsed();
    for response in votes {
        ok(response.unwrap()).await;
    }
    let body = ok(f
        .request(Method::GET, url(everyone[3].1, ""), everyone[3].0)
        .send()
        .await
        .unwrap())
    .await;
    let game = &body["game"];
    assert_eq!(
        (game["round"].as_i64(), game["phase"].as_str()),
        (Some(2), Some("collect"))
    );
    let result = &game["last_result"];
    let total: i64 = result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["votes"].as_i64().unwrap())
        .sum();
    assert_eq!(total, PLAYERS as i64, "every vote counted once");
    // A twenty-way tie goes to the earliest pick; it is now playing.
    let winners: Vec<_> = result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["winner"] == true)
        .collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(game["now_playing"]["round"], 1);
    eprintln!("20 players: picks {picked:?}, reads {read:?}, votes {voted:?}");
    // Generous on a loaded laptop; the point is that nothing serializes into seconds per player.
    assert!(picked + read + voted < std::time::Duration::from_secs(15));
    server.abort();
}

#[tokio::test]
async fn twenty_players_race_to_name_the_same_tune() {
    let f = Fixture::new().await;
    let lobby = f.lobby(PLAYERS - 1).await;
    let everyone = players(&lobby);
    let (base, server) = f.http().await;
    let url = |m: &str, suffix: &str| format!("{base}/api/lobbies/memberships/{m}/game{suffix}");
    f.start(&lobby, "name_that_tune").await;
    // Guest 0 proposes the song; the 18 other guests guess it (the host plays it).
    let (proposer, pm) = (&lobby.guests[0].0, lobby.guests[0].1.as_str());
    let mut metadata = song("fJ9rUzIMcZQ");
    metadata.title = "Queen - Bohemian Rhapsody (Official Video Remastered)".into();
    f.database
        .submit_game(proposer, pm, &request_id(proposer, "x"), &metadata, || {
            f.now()
        })
        .await
        .unwrap();
    f.advance(90);
    f.tick().await;
    assert_eq!(f.view(proposer, pm).await.phase, Phase::Guess);

    let guessers: Vec<_> = everyone[2..].to_vec();
    assert_eq!(guessers.len(), PLAYERS - 2);
    let clock = Instant::now();
    let guesses = join_all(guessers.iter().map(|(user, m)| {
        f.request(Method::POST, url(m, "/guesses"), user)
            .json(&json!({"text": "bohemian rhapsody"}))
            .send()
    }))
    .await;
    let elapsed = clock.elapsed();
    let mut points = Vec::new();
    for response in guesses {
        let body = ok(response.unwrap()).await;
        assert_eq!(body["correct"], true);
        points.push(body["points"].as_i64().unwrap());
    }
    // The last correct guess closes the round: everyone now sees the reveal.
    let reveal = f.view(proposer, pm).await;
    assert_eq!(reveal.phase, Phase::Reveal);
    let result = reveal.last_result.unwrap();
    let mut ranks: Vec<i64> = result.guesses.iter().map(|g| g.rank).collect();
    ranks.sort();
    assert_eq!(
        ranks,
        (1..=guessers.len() as i64).collect::<Vec<_>>(),
        "unique ranks"
    );
    let by_rank: Vec<i64> = {
        let mut rows = result.guesses.clone();
        rows.sort_by_key(|g| g.rank);
        rows.iter().map(|g| g.points).collect()
    };
    assert!(
        by_rank.windows(2).all(|w| w[0] >= w[1]),
        "faster never scores less: {by_rank:?}"
    );
    points.sort();
    let mut expected = by_rank.clone();
    expected.sort();
    assert_eq!(points, expected, "each response reported its own points");
    eprintln!("18 concurrent guesses: {elapsed:?}");
    server.abort();
}
