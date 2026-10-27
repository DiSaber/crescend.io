use super::*;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn malformed_selectors_and_unknown_accounts_are_rejected() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    let http = f.server().await;
    assert_eq!(
        f.request(Method::GET, http.queue("INVALID"), &host)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        f.request(Method::GET, http.queue(&membership), &host)
            .body(" ")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let unknown = crate::auth::AuthenticatedUser {
        id: i64::MAX,
        expires_at: NOW + 3600,
    };
    let response = f
        .request(Method::GET, http.queue(&membership), &unknown)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers().contains_key("www-authenticate"));
}

#[tokio::test]
async fn busy_writer_and_slow_metadata_deadlines() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.mock_metadata().await;
    let http = f.server().await;
    let url = format!("{}/entries", http.queue(&membership));
    let body = json!({"url":"https://youtu.be/dQw4w9WgXcQ", "request_id":"00000000000000000000000000000001"});
    let writer = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    let response = f
        .request(Method::POST, url.clone(), &host)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    writer.rollback().await.unwrap();
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .revision
            .value(),
        1
    );
    f.upstream.reset().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_delay(Duration::from_millis(200)).set_body_json(json!({
            "title":"Server", "author_name":"Channel", "author_url":"https://youtube.com/@channel", "thumbnail_url":"https://i.ytimg.com/image.jpg"
        }))).mount(&f.upstream).await;
    let request = f.request(Method::POST, url, &host).json(&body);
    let task = tokio::spawn(async move { request.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.upstream.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.time.store(
        NOW + i64::try_from(crate::auth::TOKEN_SECONDS).unwrap(),
        std::sync::atomic::Ordering::SeqCst,
    );
    let response = task.await.unwrap();
    assert_eq!(response.status(), 401);
    assert!(response.headers().contains_key("www-authenticate"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM queue_entries")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn failures_and_ended_generations_never_mutate() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, old) = f.guest("guest", &code).await;
    let http = f.server().await;
    let url = http.queue(&membership);
    let body = json!({"url":"https://youtu.be/dQw4w9WgXcQ", "request_id":"00000000000000000000000000000001"});
    let unavailable = f
        .request(Method::POST, format!("{url}/entries"), &host)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(unavailable.status(), 502);
    f.database
        .leave_lobby(guest.id, &old, || f.now())
        .await
        .unwrap();
    let (_, new) = {
        let view = f
            .database
            .join_lobby(guest.id, &code, || f.now())
            .await
            .unwrap();
        (guest.id, view.membership_id.unwrap().as_str().to_owned())
    };
    assert_ne!(old, new);
    assert_eq!(
        f.request(Method::GET, http.queue(&old), &guest)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let before = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap()
        .revision;
    let writer = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    assert_eq!(
        f.request(Method::POST, format!("{url}/entries"), &host)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        502
    );
    // A known provider failure cannot acquire a writer or leave a receipt.
    writer.rollback().await.unwrap();
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .revision,
        before
    );
    f.time
        .store(NOW + 86400, std::sync::atomic::Ordering::SeqCst);
    assert!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn read_submit_and_strict_requests() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, guest_membership) = f.guest("guest", &code).await;
    f.mock_metadata().await;
    let http = f.server().await;
    let url = http.queue(&membership);
    let response = f
        .request(Method::GET, url.clone(), &host)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "queue GET must be available");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let empty: Value = response.json().await.unwrap();
    assert!(empty["current"].is_null());
    assert_eq!(empty["voting"]["effective_threshold"], 1);
    for (body, content_type, status) in [
        ("{}".into(), true, 400),
        (r#"{"url":"https://youtu.be/dQw4w9WgXcQ","request_id":"00000000000000000000000000000001","title":"forged"}"#.into(), true, 400),
        (r#"{"url":"x","url":"y","request_id":"00000000000000000000000000000001"}"#.into(), true, 400),
        ("{}".into(), false, 415), ("x".repeat(4097), true, 413),
        (json!({"url":"https://example.com", "request_id":"00000000000000000000000000000001"}).to_string(), true, 422),
    ] {
        let mut request = f.request(Method::POST, format!("{url}/entries"), &host).body(body);
        if content_type { request = request.header("content-type", "application/json"); }
        let response = request.send().await.unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    assert_eq!(
        reqwest::Client::new()
            .get(&url)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        f.request(Method::GET, url.clone(), &guest)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let submitted: Value = f.request(Method::POST, format!("{}/entries", http.queue(&guest_membership)), &guest)
        .json(&json!({"url":"https://youtu.be/dQw4w9WgXcQ", "request_id":"00000000000000000000000000000001"}))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(submitted["changed"], true);
    assert_eq!(
        submitted["queue"]["current"]["video"]["title"],
        "Server title"
    );
    assert_eq!(
        submitted["queue"]["current"]["submitted_by_user_id"],
        guest.id.to_string()
    );
    assert_eq!(
        submitted["queue"]["revision"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap(),
        3
    );
}
