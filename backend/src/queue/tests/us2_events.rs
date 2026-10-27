use super::*;
use reqwest::Method;
use serde_json::json;

pub(super) async fn event(response: &mut reqwest::Response) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut buffer = String::new();
        loop {
            let bytes = response.chunk().await.unwrap().expect("stream stays open");
            buffer.push_str(&String::from_utf8(bytes.to_vec()).unwrap());
            if buffer.contains("event:") && buffer.contains("\n\n") {
                return buffer;
            }
        }
    })
    .await
    .expect("event arrives within deadline")
}

#[tokio::test]
async fn noop_and_rejected_mutations_publish_nothing() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.seed(&host, &membership, 1).await;
    let http = f.server().await;
    let url = http.queue(&membership);
    let mut hints = f.updates.subscribe();
    let before = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap()
        .revision;
    for request in [
        f.request(Method::POST, format!("{url}/entries"), &host)
            .json(&json!({"url":"https://youtu.be/dQw4w9WgXcQ","request_id":format!("{:032x}",0)})),
        f.request(Method::DELETE, format!("{url}/entries/{:032x}", 0), &host),
        f.request(Method::PUT, format!("{url}/voting-policy"), &host)
            .json(&json!({"mode":"majority"})),
        f.request(Method::POST, format!("{url}/advance"), &host)
            .json(&json!({"expected_entry_id":format!("{:032x}",0),"reason":"ended"})),
    ] {
        let response = request.send().await.unwrap();
        assert!([200, 409].contains(&response.status().as_u16()));
    }
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .revision,
        before
    );
    assert!(matches!(
        hints.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn queue_streams_end_at_auth_lobby_and_membership_deadlines() {
    for terminal in ["auth_expired", "expired", "left"] {
        let f = Fixture::new().await;
        let (_, _, code) = f.host().await;
        let (guest, generation) = f.guest("guest", &code).await;
        f.seed(&guest, &generation, 1).await;
        if terminal == "expired" {
            sqlx::query("UPDATE lobbies SET expires_at = ?")
                .bind(chrono::DateTime::from_timestamp(NOW + 1, 0).unwrap())
                .execute(&f.database.db_pool)
                .await
                .unwrap();
        }
        let http = f.server().await;
        let mut stream = f
            .request(
                Method::GET,
                format!("{}/api/lobbies/memberships/{generation}/events", http.base),
                &guest,
            )
            .send()
            .await
            .unwrap();
        event(&mut stream).await;
        if terminal == "left" {
            f.database
                .leave_lobby(guest.id, &generation, || f.now())
                .await
                .unwrap();
        } else {
            f.time.store(
                NOW + if terminal == "expired" {
                    1
                } else {
                    crate::auth::TOKEN_SECONDS as i64
                },
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        assert!(event(&mut stream).await.contains(terminal));
    }
}

#[tokio::test]
async fn queue_commits_invalidate_existing_private_stream() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.mock_metadata().await;
    let http = f.server().await;
    let mut stream = f
        .request(
            Method::GET,
            format!("{}/api/lobbies/memberships/{membership}/events", http.base),
            &host,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    assert_eq!(stream.headers()["cache-control"], "no-store");
    assert_eq!(stream.headers()["x-accel-buffering"], "no");
    let initial = event(&mut stream).await;
    assert!(initial.contains("sync_required"));
    let response = f.request(Method::POST, format!("{}/entries", http.queue(&membership)), &host)
        .json(&json!({"url":"https://youtu.be/dQw4w9WgXcQ", "request_id":"00000000000000000000000000000001"})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let changed = event(&mut stream).await;
    assert!(changed.contains("lobby_changed"));
    assert!(changed.contains(r#""revision":"2""#));
    assert!(!changed.contains("video"));
    assert!(!changed.contains("voter"));
    f.database
        .leave_lobby(host.id, &membership, || f.now())
        .await
        .unwrap();
    let ended = event(&mut stream).await;
    assert!(ended.contains("membership_ended"));
    assert!(ended.contains("closed"));
}
