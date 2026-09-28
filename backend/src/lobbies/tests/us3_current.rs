use super::*;
use serde_json::{Value, json};

async fn current(f: &Fixture, user: i64) -> Value {
    let response = Client::new()
        .get(format!("{}/api/lobbies/current", f.base))
        .bearer_auth(f.auth.jwt.issue(user).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    response.json().await.unwrap()
}

#[tokio::test]
async fn recovery_clients_privacy_and_restart() {
    let f = Fixture::new().await;
    let empty = json!({"membership_id":null,"lobby":null});
    assert_eq!(current(&f, f.user).await, empty);
    // Discard the successful creation response, as if it was lost in transit.
    assert_eq!(f.create().await.status(), 201);
    let recovered = current(&f, f.user).await;
    assert_eq!(current(&f, f.user).await, recovered);
    let member = f
        .database
        .create_user("private-member-subject")
        .await
        .unwrap()
        .id;
    let outsider = f.database.create_user("outsider").await.unwrap().id;
    let joined = f
        .database
        .join_lobby(
            member,
            recovered["lobby"]["join_code"].as_str().unwrap(),
            || NOW,
        )
        .await
        .unwrap();
    let member_view = current(&f, member).await;
    let owner_view = current(&f, f.user).await;
    assert_eq!(owner_view["lobby"], member_view["lobby"]);
    assert_eq!(member_view, serde_json::to_value(joined).unwrap());
    assert_eq!(owner_view["membership_id"], recovered["membership_id"]);
    assert_ne!(owner_view["membership_id"], member_view["membership_id"]);
    assert!(
        !member_view
            .to_string()
            .contains(owner_view["membership_id"].as_str().unwrap())
    );
    assert!(!member_view.to_string().contains("subject"));
    assert_eq!(owner_view["lobby"]["members"][0]["role"], "owner");
    assert_eq!(owner_view["lobby"]["members"][1]["role"], "member");
    let substituted: Value = f
        .client
        .get(format!(
            "{}/api/lobbies/current?user_id={}&lobby_id={}",
            f.base,
            f.user,
            recovered["lobby"]["id"].as_str().unwrap()
        ))
        .bearer_auth(f.auth.jwt.issue(outsider).unwrap())
        .json(&json!({"user_id":f.user}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(substituted, empty);
    f.database.db_pool.close().await;
    let reopened = Fixture::connect(&f.directory.path().join("test.db")).await;
    reopened.create_tables().await.unwrap();
    let restored = reopened.current_lobby(f.user, || NOW).await.unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), owner_view);
    reopened.db_pool.close().await;
}

#[tokio::test]
async fn closed_and_exact_expiry_are_null_without_cleanup() {
    let f = Fixture::new().await;
    f.create().await;
    let empty = json!({"membership_id":null,"lobby":null});
    f.time.store(NOW + 86399, Ordering::SeqCst);
    assert!(current(&f, f.user).await["lobby"].is_object());
    f.time.store(NOW + 86400, Ordering::SeqCst);
    assert_eq!(current(&f, f.user).await, empty);
    assert_eq!(f.count("lobby_memberships").await, 1);
    f.time.store(NOW, Ordering::SeqCst);
    sqlx::query("UPDATE lobbies SET closed_at = created_at")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(current(&f, f.user).await, empty);
    assert_eq!(f.count("lobby_memberships").await, 1);
}

#[tokio::test]
async fn deadline_is_rechecked_after_snapshot() {
    let f = Fixture::new().await;
    f.create().await;
    let calls = AtomicI64::new(0);
    let view = f
        .database
        .current_lobby(f.user, || {
            NOW + 86399 + calls.fetch_add(1, Ordering::SeqCst)
        })
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(view).unwrap(),
        json!({"membership_id":null,"lobby":null})
    );
}

#[tokio::test]
async fn authentication_and_storage_errors_are_private() {
    let f = Fixture::new().await;
    let expired = f.auth.jwt.issue(f.user).unwrap();
    f.time.store(NOW + 3600, Ordering::SeqCst);
    for token in [
        None,
        Some("invalid".into()),
        Some(expired),
        Some(f.auth.jwt.issue(999999).unwrap()),
    ] {
        let mut request = f
            .client
            .get(format!("{}/api/lobbies/current", f.base))
            .header("Cookie", "crescend_refresh=test");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"code":"unauthorized","error":"Authentication failed."})
        );
    }
    sqlx::query("DROP TABLE lobby_memberships")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    for (status, code) in [(500, "internal_error"), (503, "temporarily_unavailable")] {
        if status == 503 {
            f.database.db_pool.close().await;
        }
        let response = f
            .client
            .get(format!("{}/api/lobbies/current", f.base))
            .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], code);
        assert!(!body.to_string().contains("lobby_memberships"));
    }
}

#[tokio::test]
async fn snapshots_remain_coherent_during_joins() {
    let f = Fixture::new().await;
    let initial: Value = f.create().await.json().await.unwrap();
    let code = initial["lobby"]["join_code"].as_str().unwrap();
    let mut users = Vec::new();
    for i in 0..12 {
        users.push(
            f.database
                .create_user(&format!("join-{i}"))
                .await
                .unwrap()
                .id,
        );
    }
    let writer = async {
        for user in users {
            f.database.join_lobby(user, code, || NOW).await.unwrap();
        }
    };
    let reader = async {
        for _ in 0..30 {
            let view = current(&f, f.user).await;
            let members = view["lobby"]["members"].as_array().unwrap();
            assert_eq!(
                view["lobby"]["revision"]
                    .as_str()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap(),
                members.len()
            );
            let ids: Vec<i64> = members
                .iter()
                .map(|m| m["user_id"].as_str().unwrap().parse().unwrap())
                .collect();
            assert!(ids.windows(2).all(|w| w[0] < w[1]));
            assert_eq!(members.iter().filter(|m| m["role"] == "owner").count(), 1);
        }
    };
    tokio::join!(writer, reader);
    assert_eq!(current(&f, f.user).await["lobby"]["revision"], "13");
}
