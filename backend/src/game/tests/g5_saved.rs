use super::*;
use reqwest::Method;
use serde_json::{Value, json};

const RULES: &str = r#"{"prompt":"text","collect":"pick","ballot":"pairs","winner":"pair_winners","challenge":"listen","scoring":"votes_to_picker"}"#;

async fn call(
    f: &Fixture,
    method: Method,
    url: String,
    user: &AuthenticatedUser,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = f.request(method, url, user);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    assert_eq!(response.headers()["cache-control"], "no-store");
    (status, response.json().await.unwrap_or(Value::Null))
}

fn rules() -> Value {
    serde_json::from_str(RULES).unwrap()
}

async fn log_rows(f: &Fixture) -> Vec<(i64, String, String, String)> {
    sqlx::query_as(
        "SELECT account_id, event, mode_key, definition_hash FROM game_mode_log ORDER BY id",
    )
    .fetch_all(&f.database.db_pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn saved_modes_are_private_reusable_and_started_with_their_defaults() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, _) = &lobby.guests[0];
    let (base, server) = f.http().await;
    let modes = format!("{base}/api/game-modes");

    for (body, code) in [
        (json!({"name": " ", "rules": rules()}), "invalid_request"),
        (
            json!({"name": "x", "rules": rules(), "extra": true}),
            "invalid_request",
        ),
        (
            json!({"name": "x", "rules": {"prompt": "none", "collect": "pick", "ballot": "all", "winner": "pair_winners", "challenge": "listen", "scoring": "votes_to_picker"}}),
            "invalid_game_rules",
        ),
        (
            json!({"name": "x", "rules": rules(), "settings": {"vote_seconds": 7}}),
            "invalid_game_settings",
        ),
        (
            json!({"name": "x", "rules": rules(), "prompts": [{"text": "ok", "image_id": "a".repeat(32)}]}),
            "invalid_game_prompts",
        ),
        (
            json!({"name": "x", "rules": {"prompt": "image", "collect": "pick", "ballot": "all", "winner": "most_votes", "challenge": "listen", "scoring": "votes_to_picker"}, "prompts": [{"text": "a"}]}),
            "invalid_game_prompts",
        ),
    ] {
        let (status, error) = call(
            &f,
            Method::POST,
            modes.clone(),
            &lobby.host,
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            (status, error["code"].as_str()),
            (400, Some(code)),
            "{body}"
        );
    }
    assert!(log_rows(&f).await.is_empty(), "rejected saves log nothing");

    let (status, saved) = call(
        &f,
        Method::POST,
        modes.clone(),
        &lobby.host,
        Some(json!({"name": "  Rainy Duel ", "rules": rules(), "settings": {"rounds": 2, "collect_seconds": 30},
            "prompts": [{"text": "A rainy day song"}, {"text": "  A sunny one  "}, {"text": "Unused"}]})),
    )
    .await;
    assert_eq!(status, 201);
    let id = saved["id"].as_str().unwrap().to_owned();
    assert!(crate::models::queue::valid_id(&id));
    assert_eq!(
        (saved["name"].as_str(), saved["preset"].as_bool()),
        (Some("Rainy Duel"), Some(false))
    );
    assert_eq!(saved["default_prompts"][1]["text"], "A sunny one");
    let collect = saved["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "collect_seconds")
        .unwrap()
        .clone();
    assert_eq!(
        collect["default"], 30,
        "the schema serves the saved values as defaults"
    );

    let (_, listed) = call(&f, Method::GET, modes.clone(), &lobby.host, None).await;
    let listed = listed["modes"].as_array().unwrap();
    assert_eq!(listed.len(), crate::game::modes::PRESETS.len() + 1);
    assert_eq!(listed.last().unwrap()["id"], id.as_str());
    let (_, other) = call(&f, Method::GET, modes.clone(), guest, None).await;
    assert!(!other.to_string().contains(&id), "saved modes are private");
    assert!(
        !other.to_string().contains("definition_hash"),
        "the log is never served"
    );

    // Another account can neither start nor delete it.
    let other_lobby = f.lobby(0).await;
    let (status, error) = call(
        &f,
        Method::POST,
        format!(
            "{base}/api/lobbies/memberships/{}/game",
            other_lobby.host_membership
        ),
        &other_lobby.host,
        Some(json!({"mode_id": id})),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (404, Some("game_mode_unavailable"))
    );
    let response = f
        .request(Method::DELETE, format!("{modes}/{id}"), &other_lobby.host)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);

    // The owner starts it: saved settings, default prompts, overrides on top.
    let start = format!(
        "{base}/api/lobbies/memberships/{}/game",
        lobby.host_membership
    );
    let (status, started) = call(
        &f,
        Method::POST,
        start.clone(),
        &lobby.host,
        Some(json!({"mode_id": id, "settings": {"rounds": 0}})),
    )
    .await;
    assert_eq!(status, 200);
    let game = &started["game"]["game"];
    assert_eq!(game["mode_id"], id.as_str());
    assert_eq!(game["mode"]["name"], "Rainy Duel");
    assert_eq!(
        game["total_rounds"], 3,
        "rounds 0 plays every default prompt"
    );
    assert_eq!(game["prompt"]["text"], "A rainy day song");
    assert_eq!(game["phase_ends_at"].as_str().map(|_| ()), Some(()));
    call(&f, Method::DELETE, start.clone(), &lobby.host, None).await;
    let (_, restarted) = call(
        &f,
        Method::POST,
        start.clone(),
        &lobby.host,
        Some(json!({"mode_id": id, "prompts": [{"text": "Own prompt"}]})),
    )
    .await;
    assert_eq!(restarted["game"]["game"]["prompt"]["text"], "Own prompt");
    assert_eq!(restarted["game"]["game"]["total_rounds"], 1);

    let response = f
        .request(Method::DELETE, format!("{modes}/{id}"), &lobby.host)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let (_, listed) = call(&f, Method::GET, modes.clone(), &lobby.host, None).await;
    assert!(!listed.to_string().contains(&id));
    let response = f
        .request(Method::DELETE, format!("{modes}/not-hex"), &lobby.host)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    server.abort();
}

#[tokio::test]
async fn every_save_and_start_is_logged_by_definition_without_names() {
    let f = Fixture::new().await;
    let lobby = f.lobby(0).await;
    let definition =
        crate::game::custom_definition("First name", serde_json::from_value(rules()).unwrap())
            .unwrap();
    let saved = f
        .database
        .save_game_mode(&lobby.host, &definition, &[], || f.now())
        .await
        .unwrap();
    let mut renamed = definition.clone();
    renamed.name = "Other name".into();
    let start =
        |mode_id: String, definition: crate::game::modes::Definition, prompts: Vec<GamePrompt>| {
            let f = &f;
            let lobby = &lobby;
            async move {
                f.database
                    .start_game(
                        &lobby.host,
                        &lobby.host_membership,
                        &mode_id,
                        &definition,
                        &prompts,
                        || f.now(),
                    )
                    .await
                    .unwrap();
                f.database
                    .stop_game(&lobby.host, &lobby.host_membership, || f.now())
                    .await
                    .unwrap();
            }
        };
    let prompt = vec![GamePrompt {
        text: Some("p".into()),
        image_id: None,
    }];
    start(saved.id.clone(), definition.clone(), prompt.clone()).await;
    start("custom".into(), renamed, prompt).await;
    let party = (crate::game::modes::preset("party").unwrap().definition)();
    start("party".into(), party, Vec::new()).await;
    f.database
        .delete_game_mode(&lobby.host, &saved.id, || f.now())
        .await
        .unwrap();

    let rows = log_rows(&f).await;
    let events: Vec<_> = rows
        .iter()
        .map(|(_, e, k, _)| (e.as_str(), k.as_str()))
        .collect();
    assert_eq!(
        events,
        vec![
            ("created", saved.id.as_str()),
            ("started", saved.id.as_str()),
            ("started", "custom"),
            ("started", "party"),
        ],
        "deleting the mode keeps its history"
    );
    assert!(rows.iter().all(|(account, ..)| *account == lobby.host.id));
    assert_eq!(
        rows[0].3, rows[2].3,
        "the same rules and settings share a hash across names"
    );
    assert_ne!(rows[0].3, rows[3].3);
    let popular: Vec<(String, i64)> = sqlx::query_as(
        "SELECT definition_hash, count(*) FROM game_mode_log WHERE event = 'started' GROUP BY definition_hash ORDER BY count(*) DESC",
    )
    .fetch_all(&f.database.db_pool)
    .await
    .unwrap();
    assert_eq!(popular[0], (rows[0].3.clone(), 2));
}

#[tokio::test]
async fn saved_modes_are_capped_per_account() {
    let f = Fixture::new().await;
    let user = f.account("collector").await;
    let definition =
        crate::game::custom_definition("Mine", serde_json::from_value(rules()).unwrap()).unwrap();
    for _ in 0..crate::database::game_modes::MAX_SAVED_MODES {
        f.database
            .save_game_mode(&user, &definition, &[], || f.now())
            .await
            .unwrap();
    }
    assert!(matches!(
        f.database
            .save_game_mode(&user, &definition, &[], || f.now())
            .await,
        Err(crate::game::GameError::ModeLimit)
    ));
    let other = f.account("other").await;
    f.database
        .save_game_mode(&other, &definition, &[], || f.now())
        .await
        .unwrap();
    let ghost = AuthenticatedUser {
        id: 999_999,
        expires_at: NOW + 60,
    };
    assert!(matches!(
        f.database.saved_game_modes(&ghost, || f.now()).await,
        Err(crate::game::GameError::Unauthorized)
    ));
}
