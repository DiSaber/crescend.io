use super::*;
use reqwest::Method;

#[tokio::test]
async fn concurrent_membership_and_queue_snapshots_share_one_revision() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let mut work = Vec::new();
    for i in 0..8 {
        let user = f.account(&format!("guest{i}")).await;
        let db = f.database.clone();
        let code = code.clone();
        work.push(tokio::spawn(async move {
            db.join_lobby(user.id, &code, || NOW).await.unwrap();
        }));
    }
    let db = f.database.clone();
    let owner = host.id;
    let generation = membership.clone();
    work.push(tokio::spawn(async move {
        let video = crate::models::youtube::VideoMetadata {
            video_id: "dQw4w9WgXcQ".into(),
            title: "Title".into(),
            author_name: "Channel".into(),
            author_url: "https://youtube.com/@c".into(),
            thumbnail_url: "https://i.ytimg.com/t.jpg".into(),
            embed_url: "https://youtube.com/embed/dQw4w9WgXcQ".into(),
        };
        for i in 0..8 {
            db.submit_queue(
                &crate::auth::AuthenticatedUser {
                    id: owner,
                    expires_at: NOW + 3600,
                },
                &generation,
                &format!("{i:032x}"),
                &video,
                || NOW,
                || {
                    crate::models::queue::EntryId::random()
                        .map_err(|_| crate::queue::QueueError::Unavailable)
                },
            )
            .await
            .unwrap();
        }
    }));
    for _ in 0..20 {
        let queue = f
            .database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap();
        let entries = queue.upcoming.len() + usize::from(queue.current.is_some());
        assert_eq!(
            queue.revision.value(),
            1 + entries as i64 + queue.voting.active_guest_count
        );
    }
    for task in work {
        task.await.unwrap();
    }
}

#[tokio::test]
async fn dropped_publication_reconciles_and_restart_resynchronizes() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.mock_metadata().await;
    let http = f.server().await;
    let events = format!("{}/api/lobbies/memberships/{membership}/events", http.base);
    let mut stream = f
        .request(Method::GET, events.clone(), &host)
        .send()
        .await
        .unwrap();
    super::us2_events::event(&mut stream).await;
    let video = f
        .metadata
        .lookup(&crate::youtube::YouTubeVideo::parse("https://youtu.be/dQw4w9WgXcQ").unwrap())
        .await
        .unwrap();
    let result = f
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
    let changed = super::us2_events::event(&mut stream).await;
    assert!(changed.contains(r#""revision":"2""#));
    let snapshot: serde_json::Value = f
        .request(Method::GET, http.queue(&membership), &host)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["current"]["id"], result.entry_id.as_str());
    drop(stream);
    drop(http);
    let reopened = Fixture::connect(&f.directory.path().join("queue.db")).await;
    reopened.create_tables().await.unwrap();
    assert_eq!(
        reopened
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .current
            .unwrap()
            .id,
        result.entry_id
    );
    let restarted = f.server().await;
    let mut stream = f
        .request(
            Method::GET,
            format!(
                "{}/api/lobbies/memberships/{membership}/events",
                restarted.base
            ),
            &host,
        )
        .send()
        .await
        .unwrap();
    assert!(
        super::us2_events::event(&mut stream)
            .await
            .contains("sync_required")
    );
}
