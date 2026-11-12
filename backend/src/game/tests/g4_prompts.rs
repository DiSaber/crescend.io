use super::*;
use crate::game::{
    GameError,
    images::{self, tests::*},
};
use reqwest::Method;
use serde_json::{Value, json};

const BOUNDARY: &str = "crescend-test-boundary";

fn multipart(fields: &[(&str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, bytes) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"x\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn upload(
    f: &Fixture,
    base: &str,
    user: &AuthenticatedUser,
    membership: &str,
    fields: &[(&str, &[u8])],
) -> (u16, Value) {
    let response = f
        .request(
            Method::POST,
            format!("{base}/api/lobbies/memberships/{membership}/game/images"),
            user,
        )
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(multipart(fields))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    assert_eq!(response.headers()["cache-control"], "no-store");
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn start_prompts(
    f: &Fixture,
    lobby: &Lobby,
    request: Value,
) -> Result<GameSnapshot, GameError> {
    let request: StartGameRequest = serde_json::from_value(request).unwrap();
    let (mode_id, definition) = crate::game::resolve(&request)?;
    let prompts = crate::game::prompts(definition.rules.prompt, &request.prompts)?;
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
}

async fn image_root(f: &Fixture) -> std::path::PathBuf {
    images::root(&f.database).await.unwrap()
}

#[tokio::test]
async fn picture_this_uploads_strip_metadata_and_images_appear_only_in_their_round() {
    let f = Fixture::new().await;
    let lobby = f.lobby(1).await;
    let (guest, gm) = &lobby.guests[0];
    let (base, server) = f.http().await;
    let host = &lobby.host_membership;

    let (status, body) = upload(&f, &base, guest, gm, &[("image", &jpeg(10, 10))]).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("host_required"))
    );
    for (fields, status, code) in [
        (
            vec![("image", &b"GIF89a\x01\x00\x01\x00"[..])],
            415,
            "unsupported_image_type",
        ),
        (
            vec![("image", &b"definitely not an image"[..])],
            415,
            "unsupported_image_type",
        ),
        (vec![("photo", &jpeg(4, 4)[..])], 400, "invalid_request"),
        (vec![], 400, "invalid_request"),
    ] {
        let (got, body) = upload(&f, &base, &lobby.host, host, &fields).await;
        assert_eq!((got, body["code"].as_str()), (status, Some(code)));
    }
    let wide = png(images::MAX_IMAGE_SIDE + 1, 2);
    let (status, body) = upload(&f, &base, &lobby.host, host, &[("image", &wide)]).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (422, Some("image_dimensions_exceeded"))
    );
    let huge = vec![0xFF; images::MAX_IMAGE_BYTES + 10];
    let (status, body) = upload(&f, &base, &lobby.host, host, &[("image", &huge)]).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (413, Some("image_too_large"))
    );
    let one = jpeg(4, 4);
    let (status, _) = upload(
        &f,
        &base,
        &lobby.host,
        host,
        &[("image", &one), ("image", &one)],
    )
    .await;
    assert_eq!(status, 400);
    let response = f
        .request(
            Method::POST,
            format!("{base}/api/lobbies/memberships/{host}/game/images"),
            &lobby.host,
        )
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 415);

    let (status, first) = upload(
        &f,
        &base,
        &lobby.host,
        host,
        &[("image", &jpeg_with_exif(40, 20))],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(first["content_type"], "image/jpeg");
    assert_eq!(
        (first["width"].as_i64(), first["height"].as_i64()),
        (Some(20), Some(40))
    );
    let (_, second) = upload(&f, &base, &lobby.host, host, &[("image", &png(8, 8))]).await;
    assert_eq!(second["content_type"], "image/png");
    let first_id = first["image_id"].as_str().unwrap().to_owned();
    let second_id = second["image_id"].as_str().unwrap().to_owned();
    let lobby_id = f
        .database
        .game_view(&lobby.host, host, || f.now())
        .await
        .unwrap()
        .lobby_id;
    let stored = std::fs::read(
        image_root(&f)
            .await
            .join(&lobby_id)
            .join(format!("{first_id}.jpg")),
    )
    .unwrap();
    assert!(
        !stored.windows(10).any(|w| w == b"GPS-SECRET"),
        "metadata is gone on disk"
    );
    let listed: Value = f
        .request(
            Method::GET,
            format!("{base}/api/lobbies/memberships/{host}/game/images"),
            &lobby.host,
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["images"].as_array().unwrap().len(), 2);
    assert_eq!(listed["limit"], 30);
    let response = f
        .request(
            Method::GET,
            format!("{base}/api/lobbies/memberships/{gm}/game/images"),
            guest,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403, "only the host lists images");

    // Nothing is visible to guests before a round uses it.
    let get = |user: &AuthenticatedUser, membership: &str, image: &str| {
        f.request(
            Method::GET,
            format!("{base}/api/lobbies/memberships/{membership}/game/images/{image}"),
            user,
        )
        .send()
    };
    assert_eq!(get(guest, gm, &first_id).await.unwrap().status(), 404);
    let response = get(&lobby.host, host, &first_id).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "image/jpeg");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.bytes().await.unwrap().as_ref(), stored.as_slice());

    let url = format!("{base}/api/lobbies/memberships/{host}/game");
    for (body, status, code) in [
        (
            json!({"mode_id": "picture_this"}),
            400,
            "invalid_game_prompts",
        ),
        (
            json!({"mode_id": "picture_this", "prompts": [{"text": "no image"}]}),
            400,
            "invalid_game_prompts",
        ),
        (
            json!({"mode_id": "picture_this", "prompts": [{"image_id": "a".repeat(32)}]}),
            404,
            "game_image_unavailable",
        ),
        (
            json!({"mode_id": "party", "prompts": [{"text": "x"}]}),
            400,
            "invalid_game_prompts",
        ),
    ] {
        let response = f
            .request(Method::POST, url.clone(), &lobby.host)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{body}");
        let error: Value = response.json().await.unwrap();
        assert_eq!(error["code"], code);
    }
    let response = f
        .request(Method::POST, url.clone(), &lobby.host)
        .json(&json!({"mode_id": "picture_this", "prompts": [
            {"image_id": first_id, "text": "  Which song is most Mr. Bean?  "},
            {"image_id": second_id}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let started: Value = response.json().await.unwrap();
    let game = &started["game"]["game"];
    assert_eq!(game["total_rounds"], 2, "one round per prompt");
    assert_eq!(game["prompt"]["number"], 1);
    assert_eq!(game["prompt"]["text"], "Which song is most Mr. Bean?");
    assert_eq!(game["prompt"]["image_id"], first_id.as_str());
    assert!(
        !started.to_string().contains(&second_id),
        "later prompts stay hidden"
    );
    assert_eq!(get(guest, gm, &first_id).await.unwrap().status(), 200);
    assert_eq!(get(guest, gm, &second_id).await.unwrap().status(), 404);
    let response = f
        .request(
            Method::DELETE,
            format!("{base}/api/lobbies/memberships/{host}/game/images/{first_id}"),
            &lobby.host,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);

    // Round 2 reveals the second image.
    let seq = game["phase_seq"].as_i64().unwrap();
    f.database
        .advance_game(
            &lobby.host,
            host,
            &GameAdvanceRequest {
                phase_seq: Some(seq),
                play_seq: None,
            },
            || f.now(),
        )
        .await
        .unwrap();
    let view = f.view(guest, gm).await;
    assert_eq!(view.round, 2);
    assert_eq!(
        view.prompt.unwrap().image_id.as_deref(),
        Some(second_id.as_str())
    );
    assert_eq!(get(guest, gm, &second_id).await.unwrap().status(), 200);

    // Closing the lobby deletes the rows and the files.
    let folder = image_root(&f).await.join(&lobby_id);
    assert!(folder.exists());
    let response = f
        .request(
            Method::DELETE,
            format!("{base}/api/lobbies/memberships/{host}"),
            &lobby.host,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert!(!folder.exists(), "image folder removed on close");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM game_images")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    server.abort();
}

fn processed() -> images::Processed {
    images::process(&jpeg(2, 2)).unwrap()
}

#[tokio::test]
async fn image_count_is_limited_and_deletion_frees_a_slot() {
    let f = Fixture::new().await;
    let lobby = f.lobby(0).await;
    let mut ids = Vec::new();
    for _ in 0..images::MAX_IMAGES_PER_LOBBY {
        ids.push(
            f.database
                .store_game_image(&lobby.host, &lobby.host_membership, &processed(), || {
                    f.now()
                })
                .await
                .unwrap()
                .image_id,
        );
    }
    assert!(matches!(
        f.database
            .store_game_image(&lobby.host, &lobby.host_membership, &processed(), || f
                .now())
            .await,
        Err(GameError::ImageLimit)
    ));
    f.database
        .delete_game_image(&lobby.host, &lobby.host_membership, &ids[0], || f.now())
        .await
        .unwrap();
    f.database
        .delete_game_image(&lobby.host, &lobby.host_membership, &ids[0], || f.now())
        .await
        .unwrap();
    f.database
        .store_game_image(&lobby.host, &lobby.host_membership, &processed(), || {
            f.now()
        })
        .await
        .unwrap();
    let files = std::fs::read_dir(image_root(&f).await)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        std::fs::read_dir(files).unwrap().count() as i64,
        images::MAX_IMAGES_PER_LOBBY
    );
}

#[tokio::test]
async fn expired_lobbies_lose_their_images_on_the_cleanup_sweep() {
    let f = Fixture::new().await;
    let lobby = f.lobby(0).await;
    let keep = f.lobby(0).await;
    for owner in [&lobby, &keep] {
        f.database
            .store_game_image(&owner.host, &owner.host_membership, &processed(), || {
                f.now()
            })
            .await
            .unwrap();
    }
    let root = image_root(&f).await;
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    assert_eq!(f.database.sweep_game_images().await.unwrap(), 0);
    sqlx::query("UPDATE lobbies SET expires_at = ? WHERE id IN (SELECT lobby_id FROM lobby_memberships WHERE id = ?)")
        .bind(crate::models::queue::timestamp(NOW + 10).unwrap())
        .bind(&lobby.host_membership)
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    f.database
        .delete_expired_lobbies(crate::models::queue::timestamp(NOW + 10).unwrap())
        .await
        .unwrap();
    assert_eq!(f.database.sweep_game_images().await.unwrap(), 1);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
}

#[tokio::test]
async fn text_prompts_follow_their_rule_and_cap_the_rounds() {
    let f = Fixture::new().await;
    let lobby = f.lobby(0).await;
    let rules = json!({"prompt":"text","collect":"pick","ballot":"all","winner":"most_votes","challenge":"listen","scoring":"votes_to_picker"});
    let custom = |prompts: Value, settings: Value| json!({"custom": {"name": "Prompts", "rules": rules}, "prompts": prompts, "settings": settings});
    let long = "x".repeat(crate::game::MAX_PROMPT_CHARS + 1);
    let many: Vec<Value> = (0..=crate::game::MAX_PROMPTS)
        .map(|i| json!({"text": format!("p{i}")}))
        .collect();
    for prompts in [
        json!([]),
        json!([{"text": "  "}]),
        json!([{"text": long}]),
        json!([{"text": "ok", "image_id": "a".repeat(32)}]),
        json!([{"image_id": "nothex"}]),
        Value::Array(many),
    ] {
        assert!(
            matches!(
                start_prompts(&f, &lobby, custom(prompts.clone(), json!({}))).await,
                Err(GameError::InvalidPrompts)
            ),
            "{prompts}"
        );
    }
    let game = start_prompts(
        &f,
        &lobby,
        custom(json!([{"text": "A song for a rainy day"}, {"text": "Best car song"}, {"text": "Worst first-dance song"}]), json!({"rounds": 2})),
    )
    .await
    .unwrap()
    .game
    .unwrap();
    assert_eq!(game.total_rounds, Some(2), "rounds caps the prompt list");
    assert_eq!(
        game.prompt.unwrap().text.as_deref(),
        Some("A song for a rainy day")
    );
    let schema = crate::game::modes::schema(
        &crate::game::modes::preset("picture_this")
            .map(|p| (p.definition)())
            .unwrap(),
    );
    assert_eq!(schema[0].label, "Rounds (0 = one per prompt)");
}
