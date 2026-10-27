use super::*;
use crate::{database::queue::Moderation, models::queue::PolicyRequest, queue::QueueError};
use reqwest::Method;
use serde_json::{Value, json};

async fn counts(f: &Fixture) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM lobby_queue_policies),(SELECT count(*) FROM queue_entries),(SELECT count(*) FROM queue_order),(SELECT count(*) FROM queue_votes)")
        .fetch_one(&f.database.db_pool).await.unwrap()
}

#[tokio::test]
async fn every_operation_denies_foreign_generations_and_host_actions_before_targets() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&guest, &generation, 3).await;
    let http = f.server().await;
    let before = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap()
        .revision;
    let target = ids[1].as_str();
    for (method, suffix, body) in [
        (Method::GET, String::new(), None),
        (
            Method::POST,
            "/entries".into(),
            Some(json!({"url":"https://youtu.be/dQw4w9WgXcQ","request_id":format!("{:032x}",99)})),
        ),
        (Method::DELETE, format!("/entries/{target}"), None),
        (
            Method::POST,
            "/advance".into(),
            Some(json!({"expected_entry_id":ids[0].as_str(),"reason":"ended"})),
        ),
        (Method::POST, format!("/entries/{target}/votes"), None),
        (
            Method::PUT,
            "/voting-policy".into(),
            Some(json!({"mode":"fixed","count":99})),
        ),
        (
            Method::POST,
            format!("/entries/{target}/move"),
            Some(json!({"expected_revision":"1","to_index":0})),
        ),
    ] {
        let mut request = f.request(
            method.clone(),
            format!("{}{suffix}", http.queue(&membership)),
            &guest,
        );
        if let Some(body) = &body {
            request = request.json(body);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "membership_forbidden"
        );
        if method == Method::DELETE
            || suffix == "/advance"
            || suffix == "/voting-policy"
            || suffix.ends_with("/move")
        {
            let mut request = f.request(
                method,
                format!("{}{suffix}", http.queue(&generation)),
                &guest,
            );
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 403);
            assert_eq!(
                response.json::<Value>().await.unwrap()["code"],
                "host_required"
            );
        }
    }
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .revision,
        before
    );
}

#[tokio::test]
async fn populated_restart_preserves_policy_votes_receipts_and_generation() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&guest, &generation, 3).await;
    f.database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 99 }),
            || f.now(),
        )
        .await
        .unwrap();
    f.database
        .moderate_queue(
            &guest,
            &generation,
            Moderation::Vote(ids[1].as_str()),
            || f.now(),
        )
        .await
        .unwrap();
    f.database
        .veto_queue(&host, &membership, ids[0].as_str(), || f.now())
        .await
        .unwrap();
    let before = serde_json::to_value(
        f.database
            .queue_view(&guest, &generation, || f.now())
            .await
            .unwrap(),
    )
    .unwrap();
    let reopened = Fixture::connect(&f.directory.path().join("queue.db")).await;
    reopened.create_tables().await.unwrap();
    reopened.create_tables().await.unwrap();
    let after = serde_json::to_value(
        reopened
            .queue_view(&guest, &generation, || f.now())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(before, after);
    assert_eq!(counts(&f).await, (1, 3, 2, 1));
    let violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&reopened.db_pool)
        .await
        .unwrap();
    assert!(violations.is_empty());
    let receipt = reopened
        .queue_receipt(
            &guest,
            &generation,
            &format!("{:032x}", 0),
            "dQw4w9WgXcQ",
            || f.now(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(!receipt.active);
    reopened
        .leave_lobby(guest.id, &generation, || f.now())
        .await
        .unwrap();
    let new = reopened
        .join_lobby(guest.id, &code, || f.now())
        .await
        .unwrap()
        .membership_id
        .unwrap();
    assert!(matches!(
        reopened.queue_view(&guest, &generation, || f.now()).await,
        Err(QueueError::MembershipForbidden)
    ));
    assert!(
        reopened
            .queue_view(&guest, new.as_str(), || f.now())
            .await
            .unwrap()
            .current
            .unwrap()
            .has_voted
    );
}

#[tokio::test]
async fn closure_cleanup_is_atomic_and_expiry_cascades_without_early_access() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&host, &membership, 2).await;
    f.database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 99 }),
            || f.now(),
        )
        .await
        .unwrap();
    f.database
        .moderate_queue(
            &guest,
            &generation,
            Moderation::Vote(ids[0].as_str()),
            || f.now(),
        )
        .await
        .unwrap();
    f.fail_revision().await;
    assert!(
        f.database
            .leave_lobby(host.id, &membership, || f.now())
            .await
            .is_err()
    );
    assert_eq!(counts(&f).await, (1, 2, 2, 1));
    sqlx::query("DROP TRIGGER fail_queue_revision")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    f.database
        .leave_lobby(host.id, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(counts(&f).await, (0, 0, 0, 0));
    f.database.create_tables().await.unwrap();
    assert_eq!(counts(&f).await, (0, 0, 0, 0));
    let new = f
        .database
        .create_owned_lobby(host.id, || f.now())
        .await
        .unwrap()
        .membership_id
        .unwrap();
    f.seed(&host, new.as_str(), 1).await;
    f.time
        .store(NOW + 86400, std::sync::atomic::Ordering::SeqCst);
    let fresh = crate::auth::AuthenticatedUser {
        id: host.id,
        expires_at: NOW + 86401,
    };
    assert!(matches!(
        f.database
            .queue_view(&fresh, new.as_str(), || f.now())
            .await,
        Err(QueueError::MembershipForbidden)
    ));
    assert_eq!(counts(&f).await, (1, 1, 1, 0));
    f.database
        .delete_expired_lobbies(chrono::DateTime::from_timestamp(f.now(), 0).unwrap())
        .await
        .unwrap();
    assert_eq!(counts(&f).await, (0, 0, 0, 0));
    assert_eq!(crate::database::CLEANUP_INTERVAL, Duration::from_secs(60));
}

#[tokio::test]
async fn old_schema_upgrade_preserves_active_closed_and_expired_lobbies() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    for name in ["closed", "expired"] {
        let user = f.account(name).await;
        let view = f
            .database
            .create_owned_lobby(user.id, || f.now())
            .await
            .unwrap();
        if name == "closed" {
            f.database
                .leave_lobby(user.id, view.membership_id.unwrap().as_str(), || f.now())
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE lobbies SET expires_at = ? WHERE owner_user_id = ?")
                .bind(chrono::DateTime::from_timestamp(NOW + 1, 0).unwrap())
                .bind(user.id)
                .execute(&f.database.db_pool)
                .await
                .unwrap();
        }
    }
    let before: Vec<(String, i64, String, Option<String>)> =
        sqlx::query_as("SELECT id,revision,expires_at,closed_at FROM lobbies ORDER BY id")
            .fetch_all(&f.database.db_pool)
            .await
            .unwrap();
    for sql in [
        "DROP TABLE queue_votes",
        "DROP TABLE queue_order",
        "DROP TABLE queue_entries",
        "DROP TABLE lobby_queue_policies",
    ] {
        sqlx::query(sql).execute(&f.database.db_pool).await.unwrap();
    }
    f.database.create_tables().await.unwrap();
    f.database.create_tables().await.unwrap();
    let after: Vec<(String, i64, String, Option<String>)> =
        sqlx::query_as("SELECT id,revision,expires_at,closed_at FROM lobbies ORDER BY id")
            .fetch_all(&f.database.db_pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .revision
            .value(),
        1
    );
    assert_eq!(counts(&f).await, (2, 0, 0, 0));
}

#[tokio::test]
async fn overflow_and_pool_failure_are_private_and_unchanged() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    let ids = f.seed(&host, &membership, 2).await;
    sqlx::query("UPDATE lobbies SET revision = ?")
        .bind(i64::MAX)
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let http = f.server().await;
    let url = http.queue(&membership);
    let response = f
        .request(
            Method::DELETE,
            format!("{url}/entries/{}", ids[0].as_str()),
            &host,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .current
            .unwrap()
            .id,
        ids[0]
    );
    let noop: Value = f
        .request(Method::DELETE, format!("{url}/entries/{:032x}", 0), &host)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(noop["changed"], false);
    f.database.db_pool.close().await;
    let response = f.request(Method::GET, url, &host).send().await.unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "temporarily_unavailable"
    );
}

#[tokio::test]
async fn membership_ending_during_metadata_prevents_submission() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    wiremock::Mock::given(wiremock::matchers::method("GET")).respond_with(wiremock::ResponseTemplate::new(200).set_delay(Duration::from_millis(200)).set_body_json(json!({
        "title":"Title","author_name":"Channel","author_url":"https://youtube.com/@channel","thumbnail_url":"https://i.ytimg.com/image.jpg"
    }))).mount(&f.upstream).await;
    let http = f.server().await;
    let request = f
        .request(
            Method::POST,
            format!("{}/entries", http.queue(&generation)),
            &guest,
        )
        .json(&json!({"url":"https://youtu.be/dQw4w9WgXcQ","request_id":format!("{:032x}",1)}));
    let submitting = tokio::spawn(async move { request.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.upstream.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.database
        .leave_lobby(guest.id, &generation, || f.now())
        .await
        .unwrap();
    f.database
        .join_lobby(guest.id, &code, || f.now())
        .await
        .unwrap();
    assert_eq!(submitting.await.unwrap().status(), 403);
    assert_eq!(counts(&f).await, (1, 0, 0, 0));
    assert!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .current
            .is_none()
    );
}
