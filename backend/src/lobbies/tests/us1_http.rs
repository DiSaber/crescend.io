use super::*;
use serde_json::{Value, json};

#[tokio::test]
async fn creation_returns_owner_membership_and_conflicts_on_repeat() {
    let f = Fixture::new().await;
    let response = f.create().await;
    assert_eq!(response.status(), 201);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let value: Value = response.json().await.unwrap();
    let lobby = &value["lobby"];
    for id in [&value["membership_id"], &lobby["id"]] {
        let id = id.as_str().unwrap();
        assert_eq!(id.len(), 32);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
    let code = lobby["join_code"].as_str().unwrap();
    assert_eq!(code.len(), 6);
    assert!(
        code.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    );
    assert_eq!(lobby["owner_user_id"], f.user.to_string());
    assert_eq!(lobby["revision"], "1");
    assert_eq!(lobby["members"].as_array().unwrap().len(), 1);
    assert_eq!(lobby["members"][0]["user_id"], f.user.to_string());
    assert_eq!(lobby["members"][0]["role"], "owner");
    let created =
        chrono::DateTime::parse_from_rfc3339(lobby["created_at"].as_str().unwrap()).unwrap();
    let expires =
        chrono::DateTime::parse_from_rfc3339(lobby["expires_at"].as_str().unwrap()).unwrap();
    assert_eq!(created.timestamp(), NOW);
    assert_eq!(expires.timestamp() - created.timestamp(), 86_400);
    assert_eq!(lobby["members"][0]["joined_at"], lobby["created_at"]);
    assert!(!value.to_string().contains("private-provider-subject"));
    assert!(!value.to_string().contains("google_sub"));
    let repeat = f.create().await;
    assert_eq!(repeat.status(), 409);
    assert_eq!(repeat.headers()["cache-control"], "no-store");
    assert_eq!(
        repeat.json::<Value>().await.unwrap()["code"],
        "already_in_lobby"
    );
    assert_eq!(f.count("lobbies").await, 1);
    assert_eq!(f.count("lobby_memberships").await, 1);
}

#[tokio::test]
async fn authentication_precedes_body_validation_and_mutation() {
    let f = Fixture::new().await;
    let url = format!("{}/api/lobbies", f.base);
    let expired = f.auth.jwt.issue(f.user).unwrap();
    f.time.store(NOW + 3600, Ordering::SeqCst);
    for request in [
        f.client.post(&url),
        f.client
            .post(&url)
            .header("Cookie", "crescend_refresh=not-a-bearer"),
        f.client
            .post(&url)
            .bearer_auth("invalid")
            .json(&json!({"owner_user_id":"someone"})),
        f.client.post(&url).bearer_auth(expired),
        f.client
            .post(&url)
            .bearer_auth(f.auth.jwt.issue(999999).unwrap()),
    ] {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"error":"Authentication failed.","code":"unauthorized"})
        );
    }
    assert_eq!(f.count("lobbies").await, 0);
}

#[tokio::test]
async fn creation_rejects_every_nonempty_body() {
    let f = Fixture::new().await;
    for body in ["{}", " ", "{\"owner_user_id\":42}"] {
        let response = f
            .client
            .post(format!("{}/api/lobbies", f.base))
            .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "invalid_request"
        );
    }
    assert_eq!(f.count("lobbies").await, 0);
}

#[tokio::test]
async fn storage_failures_are_sanitized_and_not_partial() {
    let f = Fixture::new().await;
    sqlx::query("CREATE TRIGGER fail_creation BEFORE INSERT ON lobby_memberships BEGIN SELECT RAISE(ABORT, 'private database diagnostic'); END")
        .execute(&f.database.db_pool).await.unwrap();
    let response = f.create().await;
    assert_eq!(response.status(), 500);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(body["code"], "internal_error");
    assert!(!body.to_string().contains("diagnostic"));
    assert_eq!(f.count("lobbies").await, 0);
    assert_eq!(f.count("lobby_memberships").await, 0);
}

#[tokio::test]
async fn database_contention_is_temporary_failure() {
    let f = Fixture::new().await;
    let tx = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    let response = f.create().await;
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "temporarily_unavailable"
    );
    tx.rollback().await.unwrap();
    assert_eq!(f.count("lobbies").await, 0);
}

#[tokio::test]
async fn later_story_routes_are_not_available() {
    let f = Fixture::new().await;
    for (method, suffix) in [
        (reqwest::Method::GET, "/current"),
        (
            reqwest::Method::GET,
            "/memberships/11111111111111111111111111111111/events",
        ),
        (
            reqwest::Method::DELETE,
            "/memberships/11111111111111111111111111111111",
        ),
    ] {
        let response = f
            .client
            .request(method, format!("{}/api/lobbies{suffix}", f.base))
            .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
    }
}

#[tokio::test]
async fn exhausted_pool_returns_sanitized_temporary_failure() {
    let f = Fixture::new().await;
    let mut connections = Vec::new();
    for _ in 0..4 {
        connections.push(f.database.db_pool.acquire().await.unwrap());
    }
    let response = f.create().await;
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "temporarily_unavailable"
    );
    drop(connections);
    assert_eq!(f.count("lobbies").await, 0);
}
