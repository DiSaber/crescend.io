use super::*;
use crate::models::queue::SubmitRequest;
use wiremock::{Mock, ResponseTemplate, matchers::method};
async fn setup(f: &Fixture) -> (AuthenticatedUser, String) {
    let user = f.account("host").await;
    let view = f
        .database
        .create_owned_lobby(user.id, || NOW)
        .await
        .unwrap();
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(100)).set_body_json(serde_json::json!({"title":"Title &amp; Song","author_name":"Artist","author_url":"https://www.youtube.com/@artist","thumbnail_url":"https://example.com/t.jpg"}))).mount(&f.upstream).await;
    (user, view.membership_id.unwrap().as_str().into())
}
fn request(id: u64) -> SubmitRequest {
    SubmitRequest {
        url: "https://youtu.be/aaaaaa".into(),
        request_id: format!("{id:032x}"),
    }
}
#[tokio::test]
async fn new_submissions_replays_races_and_catalog_failure() {
    let f = Fixture::new().await;
    let (host, membership) = setup(&f).await;
    let state = f.state();
    let first = crate::queue::submit(&state, &host, &membership, request(1))
        .await
        .unwrap();
    assert!(first.changed);
    assert!(
        !crate::queue::submit(&state, &host, &membership, request(1))
            .await
            .unwrap()
            .changed
    );
    let count: i64 =
        sqlx::query_scalar("SELECT submission_count FROM catalog_songs WHERE video_id='aaaaaa'")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let (a, b) = tokio::join!(
        crate::queue::submit(&state, &host, &membership, request(2)),
        crate::queue::submit(&state, &host, &membership, request(2))
    );
    assert_ne!(a.unwrap().changed, b.unwrap().changed);
    assert_eq!(
        f.upstream.received_requests().await.unwrap().len(),
        3,
        "both racers passed the early receipt check"
    );
    assert!(
        crate::queue::submit(&state, &host, &membership, request(3))
            .await
            .unwrap()
            .changed
    );
    let count: i64 =
        sqlx::query_scalar("SELECT submission_count FROM catalog_songs WHERE video_id='aaaaaa'")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
    assert_eq!(count, 3);
    let revision = f
        .database
        .queue_view(&host, &membership, || NOW)
        .await
        .unwrap()
        .revision
        .value();
    f.database
        .record_submission(
            first.entry_id.as_str(),
            &song("aaaaaa", "Changed", "Artist"),
            NOW + 100,
        )
        .await
        .unwrap();
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || NOW)
            .await
            .unwrap()
            .revision
            .value(),
        revision
    );
    assert!(
        f.database
            .record_submission(
                first.entry_id.as_str(),
                &song("bbbbbb", "Mismatch", "Artist"),
                NOW
            )
            .await
            .is_err()
    );
    sqlx::query("CREATE TRIGGER fail_catalog BEFORE INSERT ON catalog_metadata BEGIN SELECT RAISE(FAIL,'injected'); END").execute(&f.database.db_pool).await.unwrap();
    let response = crate::queue::submit(&state, &host, &membership, request(4))
        .await
        .unwrap();
    assert!(response.changed);
    assert_eq!(response.queue.revision.value(), revision + 1);
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_submission_receipts")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(receipts, 3);
    f.database
        .delete_expired_lobbies(chrono::DateTime::from_timestamp(NOW + 86400, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT submission_count FROM catalog_songs")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        3
    );
}
#[tokio::test]
async fn count_overflow_rolls_back_receipt_and_metadata() {
    let f = Fixture::new().await;
    let entry = format!("{:032x}", 1);
    f.database
        .record_submission(&entry, &song("aaaaaa", "Original", "Artist"), NOW)
        .await
        .unwrap();
    sqlx::query("UPDATE catalog_songs SET submission_count=9223372036854775807")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert!(
        f.database
            .record_submission(
                &format!("{:032x}", 2),
                &song("aaaaaa", "New", "Artist"),
                NOW + 1
            )
            .await
            .is_err()
    );
    assert_eq!(
        f.database
            .search_catalog("Original", NOW)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_submission_receipts")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn authenticated_discovery_and_catalog_links_use_existing_host_guest_queue_flow() {
    use wiremock::matchers::query_param;
    let f = Fixture::new().await;
    let host = f.account("journey-host").await;
    let view = f
        .database
        .create_owned_lobby(host.id, || NOW)
        .await
        .unwrap();
    let host_membership = view.membership_id.unwrap().as_str().to_owned();
    let code = view.lobby.unwrap().join_code;
    let guest = f.account("journey-guest").await;
    let guest_view = f
        .database
        .join_lobby(guest.id, &code, || NOW)
        .await
        .unwrap();
    let guest_membership = guest_view.membership_id.unwrap().as_str().to_owned();
    Mock::given(method("GET"))
        .and(query_param("part", "snippet"))
        .respond_with(ResponseTemplate::new(200).set_body_json(super::discovery::provider_body()))
        .expect(1)
        .mount(&f.upstream)
        .await;
    Mock::given(method("GET")).and(query_param("format","json")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"title":"Journey song","author_name":"Artist","author_url":"https://www.youtube.com/@artist","thumbnail_url":"https://example.com/t.jpg"}))).mount(&f.upstream).await;
    let state = super::discovery::configured(&f, 90).await;
    let server = f.server(state).await;
    let client = reqwest::Client::new();
    let token = f.auth.jwt.issue(host.id).unwrap();
    let search: serde_json::Value = client
        .get(format!("{}/api/songs/search?q=cafe", server.base))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(search["discovery"]["status"], "fresh");
    let watch = search["results"][0]["watch_url"].as_str().unwrap();
    for (i, (user, membership)) in [(&host, &host_membership), (&guest, &guest_membership)]
        .into_iter()
        .enumerate()
    {
        let body = serde_json::json!({"url":watch,"request_id":format!("{:032x}",i+1)});
        let endpoint = format!(
            "{}/api/lobbies/memberships/{membership}/queue/entries",
            server.base
        );
        let response = client
            .post(&endpoint)
            .bearer_auth(f.auth.jwt.issue(user.id).unwrap())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["changed"],
            true
        );
        assert_eq!(
            client
                .post(&endpoint)
                .bearer_auth(f.auth.jwt.issue(user.id).unwrap())
                .json(&body)
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()["changed"],
            false
        );
    }
    let catalog_server = f.server(f.state()).await;
    let catalog: serde_json::Value = client
        .get(format!(
            "{}/api/songs/search?q=Journey",
            catalog_server.base
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog["results"][0]["source"], "catalog");
    let outsider = f.account("outsider").await;
    let denied = client
        .post(format!(
            "{}/api/lobbies/memberships/{host_membership}/queue/entries",
            server.base
        ))
        .bearer_auth(f.auth.jwt.issue(outsider.id).unwrap())
        .json(&serde_json::json!({"url":watch,"request_id":format!("{:032x}",99)}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT submission_count FROM catalog_songs WHERE video_id='aaaaaa'"
        )
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap(),
        2
    );
}

#[tokio::test]
async fn catalog_timeout_and_failure_preserve_http_success_and_notifications() {
    for timeout in [true, false] {
        let f = Fixture::new().await;
        let (host, membership) = setup(&f).await;
        let mut state = f.state();
        if timeout {
            Arc::get_mut(&mut state.search).unwrap().queue_record_delay =
                std::time::Duration::from_secs(2);
        } else {
            sqlx::query("CREATE TRIGGER fail_catalog BEFORE INSERT ON catalog_metadata BEGIN SELECT RAISE(FAIL,'injected'); END").execute(&f.database.db_pool).await.unwrap();
        }
        let mut events = state.lobby_updates.subscribe();
        let server = f.server(state).await;
        let client = reqwest::Client::new();
        let endpoint = format!(
            "{}/api/lobbies/memberships/{membership}/queue/entries",
            server.base
        );
        let token = f.auth.jwt.issue(host.id).unwrap();
        let body =
            serde_json::json!({"url":"https://youtu.be/aaaaaa","request_id":format!("{:032x}",1)});
        let start = std::time::Instant::now();
        let response = client
            .post(&endpoint)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let value: serde_json::Value = response.json().await.unwrap();
        assert_eq!(value["changed"], true);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        let (_, revision) = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(value["queue"]["revision"], revision.to_string());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_submission_receipts")
                .fetch_one(&f.database.db_pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            client
                .post(&endpoint)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()["changed"],
            false
        );
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
}
