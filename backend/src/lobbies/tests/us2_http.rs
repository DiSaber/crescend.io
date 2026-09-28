use super::*;
use serde_json::{Value, json};

#[tokio::test]
async fn join_storage_errors_are_sanitized() {
    let f = Fixture::new().await;
    let target: Value = f.create().await.json().await.unwrap();
    let user = f.database.create_user("joiner").await.unwrap().id;
    let body = json!({"join_code":target["lobby"]["join_code"]});
    let tx = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    error(
        join(&f, user, body.clone()).await,
        503,
        "temporarily_unavailable",
    )
    .await;
    tx.rollback().await.unwrap();
    sqlx::query("CREATE TRIGGER fail_join BEFORE INSERT ON lobby_memberships BEGIN SELECT RAISE(ABORT, 'private diagnostic'); END").execute(&f.database.db_pool).await.unwrap();
    let failure = error(join(&f, user, body).await, 500, "internal_error").await;
    assert!(!failure.to_string().contains("diagnostic"));
    assert_eq!(f.count("lobby_memberships").await, 1);
}

async fn join(f: &Fixture, user: i64, body: Value) -> Response {
    f.client
        .post(format!("{}/api/lobbies/join", f.base))
        .bearer_auth(f.auth.jwt.issue(user).unwrap())
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn error(response: Response, status: u16, code: &str) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], code);
    assert_eq!(body.as_object().unwrap().len(), 2);
    body
}

#[tokio::test]
async fn normalized_join_and_retry_return_private_coherent_views() {
    let f = Fixture::new().await;
    let owner: Value = f.create().await.json().await.unwrap();
    let user = f
        .database
        .create_user("joining-private-subject")
        .await
        .unwrap()
        .id;
    let code = owner["lobby"]["join_code"].as_str().unwrap();
    let response = join(
        &f,
        user,
        json!({"join_code": format!(" \t\r\n{}\u{000b}\u{000c}", code.to_lowercase())}),
    )
    .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let view: Value = response.json().await.unwrap();
    assert_ne!(view["membership_id"], owner["membership_id"]);
    for field in [
        "id",
        "owner_user_id",
        "created_at",
        "expires_at",
        "join_code",
    ] {
        assert_eq!(view["lobby"][field], owner["lobby"][field]);
    }
    assert_eq!(view["lobby"]["revision"], "2");
    assert_eq!(view["lobby"]["members"].as_array().unwrap().len(), 2);
    assert_eq!(view["lobby"]["members"][0]["role"], "owner");
    assert_eq!(view["lobby"]["members"][1]["role"], "member");
    assert_eq!(view["lobby"]["members"][1]["user_id"], user.to_string());
    assert!(!view.to_string().contains("subject"));
    assert!(
        !view
            .to_string()
            .contains(owner["membership_id"].as_str().unwrap())
    );
    let repeat: Value = join(&f, user, json!({"join_code":code}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(repeat, view);
    let owner_retry: Value = join(&f, f.user, json!({"join_code":code}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(owner_retry["membership_id"], owner["membership_id"]);
    assert_eq!(owner_retry["lobby"], view["lobby"]);
}

#[tokio::test]
async fn strict_input_and_body_limits_are_structured() {
    let f = Fixture::new().await;
    for body in [
        json!({}),
        json!({"join_code":null}),
        json!({"join_code":42}),
        json!({"join_code":"ABC123","user_id":1}),
        json!({"join_code":"ABC123","owner_user_id":1}),
        json!({"join_code":"ABC123","role":"owner"}),
        json!([]),
    ] {
        error(join(&f, f.user, body).await, 400, "invalid_request").await;
    }
    for code in [
        "",
        "ABC12",
        "ABC1234",
        "AB C12",
        "ÄBC123",
        "\u{00a0}ABC123",
        "ABC12!",
    ] {
        error(
            join(&f, f.user, json!({"join_code":code})).await,
            400,
            "invalid_join_code",
        )
        .await;
    }
    for body in [
        "{",
        "",
        "{\"join_code\":\"ABC123\",\"join_code\":\"ABC123\"}",
    ] {
        let r = f
            .client
            .post(format!("{}/api/lobbies/join", f.base))
            .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        error(r, 400, "invalid_request").await;
    }
    for content_type in [None, Some("text/plain")] {
        let mut r = f
            .client
            .post(format!("{}/api/lobbies/join", f.base))
            .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
            .body("{}");
        if let Some(value) = content_type {
            r = r.header("Content-Type", value);
        }
        error(r.send().await.unwrap(), 415, "unsupported_media_type").await;
    }
    let body = format!("{{\"join_code\":\"{}\"}}", "A".repeat(1024));
    let r = f
        .client
        .post(format!("{}/api/lobbies/join", f.base))
        .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    error(r, 413, "request_too_large").await;
    assert_eq!(f.count("lobby_memberships").await, 0);
}

#[tokio::test]
async fn join_requires_bearer_authentication_before_input() {
    let f = Fixture::new().await;
    let expired = f.auth.jwt.issue(f.user).unwrap();
    f.time.store(NOW + 3600, Ordering::SeqCst);
    for token in [
        None,
        Some("invalid".into()),
        Some(expired),
        Some(f.auth.jwt.issue(999999).unwrap()),
    ] {
        let mut r = f
            .client
            .post(format!("{}/api/lobbies/join", f.base))
            .header("Cookie", "crescend_refresh=test");
        if let Some(token) = token {
            r = r.bearer_auth(token);
        }
        let response = r.json(&json!({"join_code":"ABC123"})).send().await.unwrap();
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
        error(response, 401, "unauthorized").await;
    }
    assert_eq!(f.count("lobby_memberships").await, 0);
}

#[tokio::test]
async fn unavailable_codes_and_conflicts_do_not_disclose_or_mutate() {
    let f = Fixture::new().await;
    let owner: Value = f.create().await.json().await.unwrap();
    let code = owner["lobby"]["join_code"].as_str().unwrap();
    let user = f.database.create_user("other").await.unwrap().id;
    f.database.create_owned_lobby(user, || NOW).await.unwrap();
    error(
        join(&f, user, json!({"join_code":code})).await,
        409,
        "already_in_lobby",
    )
    .await;
    let unknown = error(
        join(&f, user, json!({"join_code":"000000"})).await,
        404,
        "lobby_unavailable",
    )
    .await;
    sqlx::query("UPDATE lobbies SET closed_at = created_at WHERE owner_user_id = ?")
        .bind(f.user)
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let closed = error(
        join(&f, user, json!({"join_code":code})).await,
        404,
        "lobby_unavailable",
    )
    .await;
    sqlx::query("UPDATE lobbies SET closed_at = NULL")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    f.time.store(NOW + 86400, Ordering::SeqCst);
    let expired = error(
        join(&f, user, json!({"join_code":code})).await,
        404,
        "lobby_unavailable",
    )
    .await;
    assert_eq!(unknown, closed);
    assert_eq!(unknown, expired);
    assert_eq!(f.count("lobby_memberships").await, 2);
}
