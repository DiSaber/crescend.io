use super::us4_events::{connect, frame};
use super::*;
use serde_json::{Value, json};

pub(super) async fn leave(f: &Fixture, user: i64, id: &str) -> Response {
    f.client
        .delete(format!("{}/api/lobbies/memberships/{id}", f.base))
        .bearer_auth(f.auth.jwt.issue(user).unwrap())
        .send()
        .await
        .unwrap()
}

pub(super) async fn setup(f: &Fixture) -> (Value, i64, Value) {
    let owner: Value = f.create().await.json().await.unwrap();
    let user = f.database.create_user("departing-member").await.unwrap().id;
    let member = join(f, user, &owner).await;
    (owner, user, member)
}

async fn join(f: &Fixture, user: i64, owner: &Value) -> Value {
    let response = f
        .client
        .post(format!("{}/api/lobbies/join", f.base))
        .bearer_auth(f.auth.jwt.issue(user).unwrap())
        .json(&json!({"join_code":owner["lobby"]["join_code"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

#[tokio::test]
async fn leave_validation_authentication_and_empty_success() {
    let f = Fixture::new().await;
    let (owner, user, member) = setup(&f).await;
    let id = member["membership_id"].as_str().unwrap();
    let url = format!("{}/api/lobbies/memberships/{id}", f.base);
    for request in [
        f.client.delete(&url),
        f.client
            .delete(&url)
            .header("Cookie", "crescend_refresh=test"),
        f.client.delete(&url).bearer_auth("invalid"),
    ] {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    for (target, actor, body, status, code) in [
        (
            owner["membership_id"].as_str().unwrap(),
            user,
            "",
            403,
            "membership_forbidden",
        ),
        ("INVALID", user, "", 400, "invalid_request"),
        (id, user, "{}", 400, "invalid_request"),
        (id, user, " ", 400, "invalid_request"),
        (id, i64::MAX, "", 401, "unauthorized"),
    ] {
        let response = f
            .client
            .delete(format!("{}/api/lobbies/memberships/{target}", f.base))
            .bearer_auth(f.auth.jwt.issue(actor).unwrap())
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.json::<Value>().await.unwrap()["code"], code);
    }
    assert_eq!(f.count("lobby_memberships").await, 2);
    for _ in 0..2 {
        let response = leave(&f, user, id).await;
        assert_eq!(response.status(), 204);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(response.bytes().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn departure_ends_all_clients_and_stale_retry_preserves_rejoin() {
    let f = Fixture::new().await;
    let (owner, user, member) = setup(&f).await;
    let id = member["membership_id"].as_str().unwrap();
    let mut remaining = connect(&f, owner["membership_id"].as_str().unwrap(), f.user).await;
    let mut a = connect(&f, id, user).await;
    let mut b = connect(&f, id, user).await;
    for stream in [&mut remaining, &mut a, &mut b] {
        frame(stream).await;
    }
    let mut hints = f.updates.subscribe();
    assert_eq!(leave(&f, user, id).await.status(), 204);
    assert_eq!(hints.recv().await.unwrap().1, 3);
    for stream in [&mut a, &mut b] {
        assert!(frame(stream).await.contains("\"reason\":\"left\""));
        assert!(stream.chunk().await.unwrap().is_none());
    }
    assert!(frame(&mut remaining).await.contains("\"revision\":\"3\""));
    assert_eq!(connect(&f, id, user).await.status(), 403);
    let current = f.database.current_lobby(user, || NOW).await.unwrap();
    assert!(current.membership_id.is_none() && current.lobby.is_none());
    let roster = f
        .database
        .current_lobby(f.user, || NOW)
        .await
        .unwrap()
        .lobby
        .unwrap();
    assert_eq!(roster.members.len(), 1);
    assert_eq!(roster.owner_user_id, f.user.to_string());
    let newer = join(&f, user, &owner).await;
    assert_ne!(newer["membership_id"], member["membership_id"]);
    hints.recv().await.unwrap();
    assert_eq!(leave(&f, user, id).await.status(), 204);
    assert!(hints.try_recv().is_err());
    assert_eq!(
        serde_json::to_value(f.database.current_lobby(user, || NOW).await.unwrap()).unwrap(),
        newer
    );
}

#[tokio::test]
async fn owner_closure_ends_every_generation_and_frees_every_account() {
    let f = Fixture::new().await;
    let (owner, user, member) = setup(&f).await;
    let id = owner["membership_id"].as_str().unwrap();
    let mut a = connect(&f, id, f.user).await;
    let mut b = connect(&f, member["membership_id"].as_str().unwrap(), user).await;
    frame(&mut a).await;
    frame(&mut b).await;
    assert_eq!(leave(&f, f.user, id).await.status(), 204);
    for stream in [&mut a, &mut b] {
        assert!(frame(stream).await.contains("\"reason\":\"closed\""));
        assert!(stream.chunk().await.unwrap().is_none());
    }
    assert_eq!(f.count("lobby_memberships").await, 0);
    assert_eq!(f.count("lobbies").await, 1);
    for actor in [f.user, user] {
        assert!(
            f.database
                .current_lobby(actor, || NOW)
                .await
                .unwrap()
                .lobby
                .is_none()
        );
    }
    let rejected = f
        .client
        .post(format!("{}/api/lobbies/join", f.base))
        .bearer_auth(f.auth.jwt.issue(user).unwrap())
        .json(&json!({"join_code":owner["lobby"]["join_code"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 404);
    assert_eq!(leave(&f, f.user, id).await.status(), 204);
    let replacement: Value = f.create().await.json().await.unwrap();
    join(&f, user, &replacement).await;
    assert_eq!(
        leave(&f, user, member["membership_id"].as_str().unwrap())
            .await
            .status(),
        204
    );
    assert_eq!(f.count("lobby_memberships").await, 2);
}

#[tokio::test]
async fn failed_departures_and_closures_preserve_state_and_publish_nothing() {
    for owner_leaves in [false, true] {
        let f = Fixture::new().await;
        let (owner, user, member) = setup(&f).await;
        let mut hints = f.updates.subscribe();
        sqlx::query("CREATE TRIGGER reject_departure BEFORE DELETE ON lobby_memberships BEGIN SELECT RAISE(ABORT, 'test'); END")
            .execute(&f.database.db_pool).await.unwrap();
        let (actor, view) = if owner_leaves {
            (f.user, &owner)
        } else {
            (user, &member)
        };
        let response = leave(&f, actor, view["membership_id"].as_str().unwrap()).await;
        assert_eq!(response.status(), 500);
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "internal_error"
        );
        assert!(hints.try_recv().is_err());
        assert_eq!(f.count("lobby_memberships").await, 2);
        let current = f.database.current_lobby(user, || NOW).await.unwrap();
        assert_eq!(serde_json::to_value(current).unwrap(), member);
    }
}

#[tokio::test]
async fn busy_leave_is_bounded_and_does_not_publish() {
    let f = Fixture::new().await;
    let (_, user, member) = setup(&f).await;
    let mut hints = f.updates.subscribe();
    let tx = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        leave(&f, user, member["membership_id"].as_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "temporarily_unavailable"
    );
    tx.rollback().await.unwrap();
    assert!(hints.try_recv().is_err());
    assert_eq!(f.count("lobby_memberships").await, 2);
}

#[tokio::test]
async fn reconciliation_reports_departure_and_closure_without_publication() {
    for close in [false, true] {
        let f = Fixture::new().await;
        let (owner, user, member) = setup(&f).await;
        let id = member["membership_id"].as_str().unwrap();
        let mut stream = connect(&f, id, user).await;
        frame(&mut stream).await;
        let (actor, target) = if close {
            (f.user, owner["membership_id"].as_str().unwrap())
        } else {
            (user, id)
        };
        f.database.leave_lobby(actor, target, || NOW).await.unwrap();
        let terminal = frame(&mut stream).await;
        assert!(terminal.contains(if close {
            "\"reason\":\"closed\""
        } else {
            "\"reason\":\"left\""
        }));
        assert!(stream.chunk().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn queued_old_stream_never_follows_rejoin_and_saved_expiry_survives_cleanup() {
    use super::us4_recovery::event_text;
    use futures_util::StreamExt;
    for mode in ["rejoin", "closed", "cleanup"] {
        let f = Fixture::new().await;
        let (owner, user, member) = setup(&f).await;
        let id = member["membership_id"].as_str().unwrap();
        if mode == "cleanup" {
            f.time.store(NOW + 86_399, Ordering::SeqCst);
        }
        let receiver = f.updates.subscribe();
        let bound = f
            .database
            .stream_membership(user, id)
            .await
            .unwrap()
            .unwrap();
        let lobby = bound.lobby_id.clone();
        let connection = crate::lobbies::updates::Connection::new(
            AppState {
                database: f.database.clone(),
                auth: f.auth.clone(),
                lobby_updates: f.updates.clone(),
            },
            f.auth.jwt.verify(&f.auth.jwt.issue(user).unwrap()).unwrap(),
            id.into(),
            bound,
            receiver,
        );
        let mut stream = Box::pin(connection.stream());
        let _ = stream.next().await.unwrap().unwrap();
        f.updates.publish(lobby, 999);
        let reason = match mode {
            "rejoin" => {
                assert_eq!(leave(&f, user, id).await.status(), 204);
                join(&f, user, &owner).await;
                "left"
            }
            "closed" => {
                assert_eq!(
                    leave(&f, f.user, owner["membership_id"].as_str().unwrap())
                        .await
                        .status(),
                    204
                );
                let replacement: Value = f.create().await.json().await.unwrap();
                join(&f, user, &replacement).await;
                "closed"
            }
            _ => {
                f.time.store(NOW + 86_400, Ordering::SeqCst);
                sqlx::query("DELETE FROM lobbies")
                    .execute(&f.database.db_pool)
                    .await
                    .unwrap();
                "expired"
            }
        };
        let terminal = event_text(stream.next().await.unwrap().unwrap()).await;
        assert!(terminal.contains(&format!("\"reason\":\"{reason}\"")));
        assert!(!terminal.contains("lobby_changed"));
        assert!(stream.next().await.is_none());
    }
}
