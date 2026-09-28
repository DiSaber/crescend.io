use super::*;
use serde_json::{Value, json};

pub(super) async fn connect(f: &Fixture, id: &str, user: i64) -> Response {
    f.client
        .get(format!("{}/api/lobbies/memberships/{id}/events", f.base))
        .bearer_auth(f.auth.jwt.issue(user).unwrap())
        .send()
        .await
        .unwrap()
}

pub(super) async fn frame(response: &mut Response) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        loop {
            let chunk = response
                .chunk()
                .await
                .unwrap()
                .expect("stream ended before frame");
            bytes.extend_from_slice(&chunk);
            if bytes.windows(2).any(|w| w == b"\n\n") {
                return String::from_utf8(bytes).unwrap();
            }
        }
    })
    .await
    .expect("no frame within reconciliation deadline")
}

#[tokio::test]
async fn access_headers_initial_sync_and_identical_denials() {
    let f = Fixture::new().await;
    let view: Value = f.create().await.json().await.unwrap();
    let id = view["membership_id"].as_str().unwrap();
    let other = f.database.create_user("other").await.unwrap().id;
    let url = format!("{}/api/lobbies/memberships/{id}/events", f.base);
    let denied = f.client.get(&url).send().await.unwrap();
    assert_eq!(denied.status(), 401);
    assert_eq!(denied.headers()["cache-control"], "no-store");
    let token = f.auth.jwt.issue(f.user).unwrap();
    let cookie_only = f
        .client
        .get(&url)
        .header("Cookie", "crescend_refresh=test")
        .send()
        .await
        .unwrap();
    assert_eq!(cookie_only.status(), 401);
    f.time.store(NOW + 3600, Ordering::SeqCst);
    assert_eq!(
        f.client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    f.time.store(NOW, Ordering::SeqCst);
    let malformed = connect(&f, "INVALID", f.user).await;
    assert_eq!(malformed.status(), 400);
    assert_eq!(
        malformed.json::<Value>().await.unwrap()["code"],
        "invalid_request"
    );
    let foreign = connect(&f, id, other).await;
    assert_eq!(foreign.status(), 403);
    let expected = foreign.json::<Value>().await.unwrap();
    let absent = connect(&f, &"0".repeat(32), f.user).await;
    assert_eq!(absent.status(), 403);
    assert_eq!(absent.json::<Value>().await.unwrap(), expected);
    let mut stream = connect(&f, id, f.user).await;
    assert_eq!(stream.status(), 200);
    assert!(
        stream.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(stream.headers()["cache-control"], "no-store");
    assert_eq!(stream.headers()["x-accel-buffering"], "no");
    let initial = frame(&mut stream).await;
    assert!(initial.contains("event: sync_required"));
    assert!(initial.contains("\"revision\":\"1\""));
    assert!(!initial.contains(view["lobby"]["join_code"].as_str().unwrap()));
    sqlx::query("UPDATE lobbies SET closed_at = created_at")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(
        connect(&f, id, f.user).await.json::<Value>().await.unwrap(),
        expected
    );
    f.time.store(NOW + 86_400, Ordering::SeqCst);
    assert_eq!(
        connect(&f, id, f.user).await.json::<Value>().await.unwrap(),
        expected
    );
}

#[tokio::test]
async fn joins_fan_out_reconnect_and_disconnect_preserves_membership() {
    let f = Fixture::new().await;
    let view: Value = f.create().await.json().await.unwrap();
    let id = view["membership_id"].as_str().unwrap();
    let mut a = connect(&f, id, f.user).await;
    let mut b = connect(&f, id, f.user).await;
    frame(&mut a).await;
    frame(&mut b).await;
    let other = f.database.create_user("joiner").await.unwrap().id;
    let joined = f
        .client
        .post(format!("{}/api/lobbies/join", f.base))
        .bearer_auth(f.auth.jwt.issue(other).unwrap())
        .json(&json!({"join_code":view["lobby"]["join_code"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(joined.status(), 200);
    for s in [&mut a, &mut b] {
        let update = frame(s).await;
        assert!(update.contains("event: lobby_changed"));
        assert!(update.contains("\"revision\":\"2\""));
        assert!(!update.contains("members"));
    }
    drop(a);
    let mut reconnected = connect(&f, id, f.user).await;
    assert!(frame(&mut reconnected).await.contains("sync_required"));
    assert_eq!(f.count("lobby_memberships").await, 2);
}

#[tokio::test]
async fn idle_streams_stop_at_auth_and_lobby_deadlines_without_cleanup() {
    for lobby_expiry in [false, true] {
        let f = Fixture::new().await;
        let view: Value = f.create().await.json().await.unwrap();
        if lobby_expiry {
            f.time.store(NOW + 86_399, Ordering::SeqCst);
        }
        let mut stream = connect(&f, view["membership_id"].as_str().unwrap(), f.user).await;
        frame(&mut stream).await;
        f.time.store(
            if lobby_expiry {
                NOW + 86_400
            } else {
                NOW + 3600
            },
            Ordering::SeqCst,
        );
        let terminal = frame(&mut stream).await;
        assert!(terminal.contains(if lobby_expiry {
            "\"reason\":\"expired\""
        } else {
            "auth_expired"
        }));
        assert!(stream.chunk().await.unwrap().is_none());
        assert_eq!(f.count("lobby_memberships").await, 1);
    }
}

#[tokio::test]
async fn keepalive_and_cross_lobby_isolation() {
    let f = Fixture::new().await;
    let view: Value = f.create().await.json().await.unwrap();
    let mut stream = connect(&f, view["membership_id"].as_str().unwrap(), f.user).await;
    frame(&mut stream).await;
    let outsider = f.database.create_user("isolated").await.unwrap().id;
    let separate = f
        .database
        .create_owned_lobby(outsider, || NOW)
        .await
        .unwrap();
    let lobby = separate.lobby.unwrap();
    f.updates.publish(lobby.id, 999);
    // A comment is the only frame due on this unchanged stream.
    let chunk = tokio::time::timeout(Duration::from_secs(17), stream.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(std::str::from_utf8(&chunk).unwrap(), ": keepalive\n\n");
}

#[tokio::test]
async fn only_successful_new_joins_publish_and_repeats_are_silent() {
    let f = Fixture::new().await;
    let mut hints = f.updates.subscribe();
    let view: Value = f.create().await.json().await.unwrap();
    assert_eq!(hints.recv().await.unwrap().1, 1);
    let other = f.database.create_user("publisher").await.unwrap().id;
    let join = || {
        f.client
            .post(format!("{}/api/lobbies/join", f.base))
            .bearer_auth(f.auth.jwt.issue(other).unwrap())
            .json(&json!({"join_code":view["lobby"]["join_code"]}))
    };
    sqlx::query("CREATE TRIGGER reject_join BEFORE INSERT ON lobby_memberships BEGIN SELECT RAISE(ABORT, 'test'); END").execute(&f.database.db_pool).await.unwrap();
    assert_eq!(join().send().await.unwrap().status(), 500);
    assert!(matches!(
        hints.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    sqlx::query("DROP TRIGGER reject_join")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(join().send().await.unwrap().status(), 200);
    assert_eq!(hints.recv().await.unwrap().1, 2);
    assert_eq!(join().send().await.unwrap().status(), 200);
    assert!(matches!(
        hints.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn pre_stream_storage_errors_and_nonexistent_accounts_are_sanitized() {
    let f = Fixture::new().await;
    let id = "1".repeat(32);
    assert_eq!(connect(&f, &id, i64::MAX).await.status(), 401);
    sqlx::query("DROP TABLE lobby_memberships")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let error = connect(&f, &id, f.user).await;
    assert_eq!(error.status(), 500);
    assert_eq!(
        error.json::<Value>().await.unwrap()["code"],
        "internal_error"
    );
    f.database.db_pool.close().await;
    assert_eq!(connect(&f, &id, f.user).await.status(), 503);
}
