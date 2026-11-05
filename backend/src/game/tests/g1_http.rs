use super::*;
use reqwest::Method;
use serde_json::{Value, json};

async fn body(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    assert_eq!(
        response.headers()["cache-control"],
        "no-store",
        "every game response is no-store"
    );
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn game_modes_list_presets_with_their_settings_schema() {
    let f = Fixture::new().await;
    let (base, server) = f.http().await;
    let user = f.account("someone").await;
    let (status, modes) = body(
        f.request(Method::GET, format!("{base}/api/game-modes"), &user)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let party = &modes["modes"][0];
    assert_eq!(party["id"], "party");
    assert_eq!(party["preset"], true);
    assert_eq!(party["rules"]["ballot"], "all");
    let rounds = party["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "rounds")
        .unwrap();
    assert_eq!(rounds["type"], "integer");
    assert_eq!(rounds["default"], 5);
    assert_eq!(rounds["min"], 0);
    let anonymous = reqwest::get(format!("{base}/api/game-modes"))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    server.abort();
}

#[tokio::test]
async fn http_party_flow_publishes_each_commit_on_the_lobby_stream() {
    let f = Fixture::new().await;
    f.mock_metadata(
        "Queen - Bohemian Rhapsody (Official Video Remastered)",
        "Queen Official",
    )
    .await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    let (base, server) = f.http().await;
    let game = |m: &str, suffix: &str| format!("{base}/api/lobbies/memberships/{m}/game{suffix}");
    let mut hints = f.updates.subscribe();

    let (status, error) = body(
        f.request(Method::POST, game(gm, ""), guest)
            .json(&json!({"mode_id": "party"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (403, Some("host_required"))
    );
    let (status, error) = body(
        f.request(Method::POST, game(&lobby.host_membership, ""), &lobby.host)
            .json(&json!({"mode_id": "party", "extra": 1}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (400, Some("invalid_request"))
    );
    let (status, error) = body(
        f.request(Method::POST, game(&lobby.host_membership, ""), &lobby.host)
            .body(r#"{"mode_id":"party"}"#)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (415, Some("unsupported_media_type"))
    );
    let (status, empty) = body(
        f.request(Method::GET, game(gm, ""), guest)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert!(empty["game"].is_null());

    let (status, started) = body(
        f.request(Method::POST, game(&lobby.host_membership, ""), &lobby.host)
            .json(&json!({"mode_id": "party"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    let revision: i64 = started["game"]["revision"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(hints.recv().await.unwrap().1, revision);
    assert_eq!(started["game"]["game"]["phase"], "collect");
    assert_eq!(started["game"]["game"]["mode"]["name"], "Party");

    let (status, error) = body(
        f.request(Method::POST, game(gm, "/submissions"), guest)
            .json(&json!({"url": "https://example.com/x", "request_id": "1".repeat(32)}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((status, error["code"].as_str()), (422, Some("invalid_url")));
    let (status, submitted) = body(
        f.request(Method::POST, game(gm, "/submissions"), guest)
            .json(&json!({"url": "https://youtu.be/fJ9rUzIMcZQ", "request_id": "1".repeat(32)}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(submitted["changed"], true);
    assert_eq!(
        submitted["game"]["game"]["my_submissions"][0]["song"]["title"],
        "Queen - Bohemian Rhapsody (Official Video Remastered)"
    );
    let revision: i64 = submitted["game"]["revision"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(hints.recv().await.unwrap().1, revision);
    let (_, retried) = body(
        f.request(Method::POST, game(gm, "/submissions"), guest)
            .json(&json!({"url": "https://youtu.be/fJ9rUzIMcZQ", "request_id": "1".repeat(32)}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(retried["changed"], false);
    assert_eq!(retried["submission_id"], submitted["submission_id"]);
    assert_eq!(
        f.upstream.received_requests().await.unwrap().len(),
        1,
        "retries reuse the receipt without another metadata lookup"
    );

    let (_, host_pick) = body(
        f.request(Method::POST, game(&lobby.host_membership, "/submissions"), &lobby.host)
            .json(&json!({"url": "https://www.youtube.com/watch?v=hTWKbfoikeg", "request_id": "2".repeat(32)}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(host_pick["game"]["game"]["phase"], "vote");
    let (status, error) = body(
        f.request(Method::POST, game(gm, "/votes"), guest)
            .json(&json!({"submission_id": "f".repeat(32)}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (404, Some("game_submission_unavailable"))
    );
    let (status, voted) = body(
        f.request(Method::POST, game(gm, "/votes"), guest)
            .json(&json!({"submission_id": host_pick["submission_id"]}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(voted["changed"], true);

    let (status, error) = body(
        f.request(
            Method::POST,
            game(&lobby.host_membership, "/advance"),
            &lobby.host,
        )
        .json(&json!({"phase_seq": 1, "play_seq": 1}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (400, Some("invalid_request"))
    );
    let seq = voted["game"]["game"]["phase_seq"].clone();
    let (status, error) = body(
        f.request(
            Method::POST,
            game(&lobby.host_membership, "/advance"),
            &lobby.host,
        )
        .json(&json!({"phase_seq": 99}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (409, Some("game_phase_changed"))
    );
    let (status, advanced) = body(
        f.request(
            Method::POST,
            game(&lobby.host_membership, "/advance"),
            &lobby.host,
        )
        .json(&json!({"phase_seq": seq}))
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        advanced["game"]["game"]["now_playing"]["song"]["video_id"],
        "hTWKbfoikeg"
    );

    let outsider = f.account("outsider").await;
    let (status, error) = body(
        f.request(Method::GET, game(gm, ""), &outsider)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (403, Some("membership_forbidden"))
    );
    let (status, _) = body(
        f.request(Method::GET, game("nothex", ""), guest)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400);
    let (status, stopped) = body(
        f.request(
            Method::DELETE,
            game(&lobby.host_membership, ""),
            &lobby.host,
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(stopped["game"]["game"]["status"], "ended");
    server.abort();
}

#[test]
fn game_routes_are_documented() {
    use utoipa::OpenApi;
    let doc = serde_json::to_value(crate::routes::ApiDoc::openapi()).unwrap();
    let paths = doc["paths"].as_object().unwrap();
    for (path, method) in [
        ("/api/game-modes", "get"),
        ("/api/lobbies/memberships/{membership_id}/game", "get"),
        ("/api/lobbies/memberships/{membership_id}/game", "post"),
        ("/api/lobbies/memberships/{membership_id}/game", "delete"),
        (
            "/api/lobbies/memberships/{membership_id}/game/advance",
            "post",
        ),
        (
            "/api/lobbies/memberships/{membership_id}/game/submissions",
            "post",
        ),
        (
            "/api/lobbies/memberships/{membership_id}/game/votes",
            "post",
        ),
    ] {
        let operation = &paths[path][method];
        assert_eq!(
            operation["security"][0]["bearer_auth"],
            json!([]),
            "{method} {path}"
        );
        for status in ["200", "400", "401", "500", "503"] {
            assert!(
                operation["responses"][status].is_object(),
                "{path} {status}"
            );
        }
    }
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    for name in [
        "GameSnapshot",
        "GameView",
        "Phase",
        "Rules",
        "SettingSchema",
        "NowPlaying",
    ] {
        assert!(schemas.contains_key(name), "missing schema {name}");
    }
}
