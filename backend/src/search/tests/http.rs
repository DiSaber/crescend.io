use super::*;

#[tokio::test]
async fn authenticated_outside_lobby_strict_input_and_no_store() {
    let f = Fixture::new().await;
    let user = f.account("outside").await;
    let server = f.server(f.state()).await;
    let client = reqwest::Client::new();
    let endpoint = format!("{}/api/songs/search", server.base);
    for auth in [None, Some("invalid")] {
        let mut request = client.get(format!("{endpoint}?q=test"));
        if let Some(token) = auth {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let token = f.auth.jwt.issue(user.id).unwrap();
    for query in [
        "",
        "q=",
        "q=%20",
        "q=a&q=b",
        "q=a&more=1",
        "q=a&x=b",
        "q=a&more=true&more=false",
        "q=%FF",
        "q=%xy",
    ] {
        let response = client
            .get(format!("{endpoint}?{query}"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{query}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["code"],
            "invalid_search_request"
        );
    }
    for (length, expected) in [(200, 200), (201, 400)] {
        let url = url::Url::parse_with_params(&endpoint, &[("q", "é".repeat(length))]).unwrap();
        assert_eq!(
            client
                .get(url)
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    assert_eq!(
        client
            .get(format!("{endpoint}?q=valid"))
            .bearer_auth(&token)
            .body("body")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .get(format!("{endpoint}?q={}", "a".repeat(8191)))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        414
    );
    assert_eq!(
        client
            .get(format!("{endpoint}?q={}", "a".repeat(8190)))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let response = client
        .get(format!("{endpoint}?q=test"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({"results":[],"discovery":{"status":"missing_credentials"}})
    );
    assert!(f.upstream.received_requests().await.unwrap().is_empty());
    sqlx::query("DROP TABLE catalog_metadata")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{endpoint}?q=test"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
}

#[tokio::test]
async fn sufficient_catalog_avoids_provider_and_exact_response_schema() {
    let f = Fixture::new().await;
    let rows:Vec<_>=(0..12).map(|i|serde_json::json!({"video_id":format!("{i:06}"),"title":"known song","artist":"Artist"})).collect();
    f.database
        .import_starters(&serde_json::to_string(&rows).unwrap(), f.now())
        .await
        .unwrap();
    let state = super::discovery::configured(&f, 90).await;
    let server = f.server(state).await;
    let user = f.account("catalog-hit").await;
    let response = reqwest::Client::new()
        .get(format!("{}/api/songs/search?q=known", server.base))
        .bearer_auth(f.auth.jwt.issue(user.id).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["discovery"]["status"], "not_needed");
    assert_eq!(body["results"].as_array().unwrap().len(), 10);
    let keys: std::collections::BTreeSet<_> = body["results"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "artist",
            "source",
            "thumbnail_url",
            "title",
            "video_id",
            "watch_url"
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(body["results"][0]["source"], "catalog");
    assert!(f.upstream.received_requests().await.unwrap().is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM song_search_daily_usage")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn malformed_provider_transport_degrades_without_secret_exposure() {
    use wiremock::{Mock, ResponseTemplate, matchers::method};
    for template in [
        ResponseTemplate::new(500).set_body_string("private-test-key"),
        ResponseTemplate::new(200).set_body_string("not json private-test-key"),
        ResponseTemplate::new(200).set_body_string("x".repeat(262145)),
        ResponseTemplate::new(302).insert_header("Location", "https://example.com/redirect"),
    ] {
        let f = Fixture::new().await;
        Mock::given(method("GET"))
            .respond_with(template)
            .mount(&f.upstream)
            .await;
        let state = super::discovery::configured(&f, 90).await;
        let user = f.account("failure").await;
        let server = f.server(state).await;
        let response = reqwest::Client::new()
            .get(format!("{}/api/songs/search?q=query", server.base))
            .bearer_auth(f.auth.jwt.issue(user.id).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let text = response.text().await.unwrap();
        assert!(!text.contains("private-test-key"));
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["discovery"]["status"], "temporarily_unavailable");
        assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT sum(attempts) FROM song_search_daily_usage")
                .fetch_one(&f.database.db_pool)
                .await
                .unwrap(),
            1
        );
    }
}
