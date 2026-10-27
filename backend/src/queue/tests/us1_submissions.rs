use super::*;
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn prefeature_upgrade_and_coherent_concurrent_appends() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    for statement in [
        "DROP TABLE queue_votes",
        "DROP TABLE queue_order",
        "DROP TABLE queue_entries",
        "DROP TABLE lobby_queue_policies",
    ] {
        sqlx::query(statement)
            .execute(&f.database.db_pool)
            .await
            .unwrap();
    }
    f.database.create_tables().await.unwrap();
    f.database.create_tables().await.unwrap();
    let initial = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(initial.revision.value(), 1);
    assert!(initial.current.is_none());
    f.mock_metadata().await;
    let video = Arc::new(
        f.metadata
            .lookup(&crate::youtube::YouTubeVideo::parse("https://youtu.be/dQw4w9WgXcQ").unwrap())
            .await
            .unwrap(),
    );
    let mut writers = Vec::new();
    for i in 1..=16 {
        let db = f.database.clone();
        let video = video.clone();
        let membership = membership.clone();
        let user_id = host.id;
        writers.push(tokio::spawn(async move {
            db.submit_queue(
                &crate::auth::AuthenticatedUser {
                    id: user_id,
                    expires_at: NOW + 3600,
                },
                &membership,
                &format!("{i:032x}"),
                &video,
                || NOW,
                || {
                    crate::models::queue::EntryId::random()
                        .map_err(|_| crate::queue::QueueError::Unavailable)
                },
            )
            .await
            .unwrap()
        }));
    }
    for _ in 0..16 {
        let view = f
            .database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap();
        let entries = view.upcoming.len() + usize::from(view.current.is_some());
        assert_eq!(view.revision.value(), 1 + entries as i64);
    }
    for writer in writers {
        writer.await.unwrap();
    }
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .upcoming
            .len(),
        15
    );
}

#[tokio::test]
async fn allocation_exhaustion_and_rollback_preserve_queue() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.mock_metadata().await;
    let video = f
        .metadata
        .lookup(&crate::youtube::YouTubeVideo::parse("https://youtu.be/dQw4w9WgXcQ").unwrap())
        .await
        .unwrap();
    let first = f
        .database
        .submit_queue(
            &host,
            &membership,
            "00000000000000000000000000000001",
            &video,
            || f.now(),
            || {
                crate::models::queue::EntryId::random()
                    .map_err(|_| crate::queue::QueueError::Unavailable)
            },
        )
        .await
        .unwrap();
    let mut attempts = 0;
    let failed = f
        .database
        .submit_queue(
            &host,
            &membership,
            "00000000000000000000000000000002",
            &video,
            || f.now(),
            || {
                attempts += 1;
                Ok(first.entry_id.clone())
            },
        )
        .await;
    assert!(matches!(failed, Err(crate::queue::QueueError::Unavailable)));
    assert_eq!(attempts, 8);
    f.fail_revision().await;
    let failed = f
        .database
        .submit_queue(
            &host,
            &membership,
            "00000000000000000000000000000003",
            &video,
            || f.now(),
            || {
                crate::models::queue::EntryId::random()
                    .map_err(|_| crate::queue::QueueError::Unavailable)
            },
        )
        .await;
    assert!(failed.is_err());
    let view = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(view.revision.value(), 2);
    assert!(view.upcoming.is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM queue_entries")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn durable_receipts_duplicates_concurrency_and_restart() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.mock_metadata().await;
    let http = f.server().await;
    let url = http.queue(&membership);
    let body = json!({"url":"https://youtu.be/dQw4w9WgXcQ", "request_id":"00000000000000000000000000000001"});
    let (a, b) = tokio::join!(
        f.request(Method::POST, format!("{url}/entries"), &host)
            .json(&body)
            .send(),
        f.request(Method::POST, format!("{url}/entries"), &host)
            .json(&body)
            .send()
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.status(), 200, "submission must accept a valid video");
    assert_eq!(b.status(), 200);
    let a: Value = a.json().await.unwrap();
    let b: Value = b.json().await.unwrap();
    assert_eq!(a["entry_id"], b["entry_id"]);
    assert_ne!(a["changed"], b["changed"]);
    let duplicate: Value = f
        .request(Method::POST, format!("{url}/entries"), &host)
        .json(&json!({
            "url":"https://youtu.be/dQw4w9WgXcQ", "request_id":"00000000000000000000000000000002"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(a["entry_id"], duplicate["entry_id"]);
    assert_eq!(duplicate["queue"]["upcoming"].as_array().unwrap().len(), 1);
    let conflict = f
        .request(Method::POST, format!("{url}/entries"), &host)
        .json(&json!({
            "url":"https://youtu.be/abcdefghijk", "request_id":"00000000000000000000000000000001"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    f.upstream.reset().await;
    let retry: Value = f
        .request(Method::POST, format!("{url}/entries"), &host)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retry["changed"], false);
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 0);
    drop(http);
    let reopened = Fixture::connect(&f.directory.path().join("queue.db")).await;
    reopened.create_tables().await.unwrap();
    reopened.create_tables().await.unwrap();
    let ids: Vec<String> = sqlx::query_scalar("SELECT entry_id FROM queue_order ORDER BY position")
        .fetch_all(&reopened.db_pool)
        .await
        .unwrap();
    assert_eq!(
        ids,
        vec![
            a["entry_id"].as_str().unwrap(),
            duplicate["entry_id"].as_str().unwrap()
        ]
    );
}
