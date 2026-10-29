use super::*;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{header, method, path, query_param},
};
pub(super) fn provider_body() -> serde_json::Value {
    serde_json::json!({"items":[{"id":{"kind":"youtube#video","videoId":"aaaaaa"},"snippet":{"title":"Caf&eacute; &amp;amp; music","channelTitle":"Artist &#39;One&#39;","thumbnails":{"high":{"url":"https://example.com/high.jpg"}}}}]})
}
pub(super) async fn configured(f: &Fixture, limit: i64) -> AppState {
    let mut state = f.state();
    state.search = super::super::SearchService::new(
        super::super::config::SearchConfig {
            api_key: Some("private-test-key".into()),
            daily_limit: limit,
        },
        f.auth.clock.clone(),
        crate::youtube::search::SearchClient::for_test(f.upstream.uri().parse().unwrap()),
    );
    state
}
#[tokio::test]
async fn fresh_cache_restart_filters_decoding_and_empty_success() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("part", "snippet"))
        .and(query_param("type", "video"))
        .and(query_param("videoEmbeddable", "true"))
        .and(query_param("videoCategoryId", "10"))
        .and(query_param("maxResults", "10"))
        .and(query_param("order", "relevance"))
        .and(query_param("q", "cafe"))
        .and(header("x-goog-api-key", "private-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(provider_body()))
        .expect(1)
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 90).await;
    let first = state
        .search
        .search(&f.database, "cafe", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        first.discovery.status,
        crate::models::search::DiscoveryStatus::Fresh
    );
    assert_eq!(first.results[0].song.title, "Café &amp; music");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let reopened = Fixture::connect(&f.directory.path().join("search.db")).await;
    let cached = state
        .search
        .search(&reopened, "cafe", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        cached.discovery.status,
        crate::models::search::DiscoveryStatus::Cached
    );
    assert_eq!(cached.results.len(), 1);
    assert_eq!(cached.results[0].song.title, "Café &amp; music");
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn equivalent_overlap_and_leader_disconnect_share_one_attempt() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(150))
                .set_body_json(provider_body()),
        )
        .expect(1)
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 90).await;
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let s = state.clone();
        jobs.spawn(async move {
            s.search
                .search(&s.database, "cafe", true, NOW)
                .await
                .unwrap()
        });
    }
    while let Some(r) = jobs.join_next().await {
        assert_eq!(
            r.unwrap().discovery.status,
            crate::models::search::DiscoveryStatus::Fresh
        );
    }
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn errors_consume_allowance_cache_outages_suppress_requests() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_string("private-test-key"))
        .expect(1)
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 1).await;
    let first = state
        .search
        .search(&f.database, "one", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        first.discovery.status,
        crate::models::search::DiscoveryStatus::TemporarilyUnavailable
    );
    let second = state
        .search
        .search(&f.database, "two", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        second.discovery.status,
        crate::models::search::DiscoveryStatus::DailyLimitReached
    );
    sqlx::query("DROP TABLE song_search_cache")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let third = state
        .search
        .search(&f.database, "three", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        third.discovery.status,
        crate::models::search::DiscoveryStatus::TemporarilyUnavailable
    );
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn provider_failures_empty_cache_and_cache_precedence() {
    use crate::models::search::DiscoveryStatus as D;
    for (body, status) in [
        (serde_json::json!({"items":[]}), D::Fresh),
        (serde_json::json!({"items":[{}]}), D::TemporarilyUnavailable),
        (
            serde_json::json!({"unexpected":[]}),
            D::TemporarilyUnavailable,
        ),
    ] {
        let f = Fixture::new().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&f.upstream)
            .await;
        let state = configured(&f, 90).await;
        let response = state
            .search
            .search(&f.database, "empty", true, NOW)
            .await
            .unwrap();
        assert_eq!(response.discovery.status, status);
        if status == D::Fresh {
            assert!(
                f.database
                    .cached_search("empty", NOW)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_empty()
            );
            let disabled = f.state();
            assert_eq!(
                disabled
                    .search
                    .search(&f.database, "empty", true, NOW)
                    .await
                    .unwrap()
                    .discovery
                    .status,
                D::Cached
            );
            assert_eq!(
                configured(&f, 0)
                    .await
                    .search
                    .search(&f.database, "empty", true, NOW)
                    .await
                    .unwrap()
                    .discovery
                    .status,
                D::Cached
            );
        } else {
            assert!(
                f.database
                    .cached_search("empty", NOW)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
    }
}
#[tokio::test]
async fn failed_persistence_shared_result_then_later_retry_and_cleanup() {
    use crate::models::search::DiscoveryStatus as D;
    let f = Fixture::new().await;
    sqlx::query("CREATE TRIGGER fail_cache BEFORE INSERT ON song_search_cache BEGIN SELECT RAISE(FAIL,'injected'); END").execute(&f.database.db_pool).await.unwrap();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(100))
                .set_body_json(provider_body()),
        )
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 90).await;
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let s = state.clone();
        jobs.spawn(async move {
            s.search
                .search(&s.database, "cafe", true, NOW)
                .await
                .unwrap()
        });
    }
    while let Some(r) = jobs.join_next().await {
        let response = r.unwrap();
        assert_eq!(response.discovery.status, D::Fresh);
        assert_eq!(response.results.len(), 1);
    }
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
    assert!(state.search.flights.lock().unwrap().is_empty());
    assert!(
        f.database
            .cached_search("cafe", NOW)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.database
            .search_catalog("cafe", NOW)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("DROP TRIGGER fail_cache")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(
        state
            .search
            .search(&f.database, "cafe", true, NOW)
            .await
            .unwrap()
            .discovery
            .status,
        D::Fresh
    );
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sum(attempts) FROM song_search_daily_usage")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        2
    );
}
#[tokio::test]
async fn leader_disconnect_does_not_cancel_attempt_or_waiters() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(200))
                .set_body_json(provider_body()),
        )
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 90).await;
    let s = state.clone();
    let leader =
        tokio::spawn(async move { s.search.search(&s.database, "query", true, NOW).await });
    for _ in 0..100 {
        if !f.upstream.received_requests().await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
    leader.abort();
    let response = state
        .search
        .search(&f.database, "query", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        response.discovery.status,
        crate::models::search::DiscoveryStatus::Fresh
    );
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
    assert!(state.search.flights.lock().unwrap().is_empty());
}

#[tokio::test]
async fn normalized_queries_expiry_and_invalid_item_thumbnail_fallback() {
    use crate::models::search::DiscoveryStatus as D;
    let f = Fixture::new().await;
    let body = serde_json::json!({"items":[{}, {"id":{"kind":"youtube#video","videoId":"aaaaaa"},"snippet":{"title":"Once &amp;amp;","channelTitle":"Channel &#x27;name&#x27;","thumbnails":{"high":{"url":"http://bad.example/high"},"medium":{"url":"https://example.com/medium"}}}},{"id":{"kind":"youtube#channel","videoId":"bbbbbb"},"snippet":{"title":"Ignore","channelTitle":"Channel"}}]});
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 90).await;
    let first = state
        .search
        .search(&f.database, " CAFÉ   Music ", true, NOW)
        .await
        .unwrap();
    assert_eq!(first.results.len(), 1);
    assert_eq!(
        first.results[0].song.thumbnail_url.as_deref(),
        Some("https://example.com/medium")
    );
    assert_eq!(first.results[0].song.artist, "Channel 'name'");
    assert_eq!(
        state
            .search
            .search(&f.database, "cafe music", true, NOW)
            .await
            .unwrap()
            .discovery
            .status,
        D::Cached
    );
    f.time.store(NOW + 604800, Ordering::SeqCst);
    assert_eq!(
        state
            .search
            .search(&f.database, "café music", true, f.now())
            .await
            .unwrap()
            .discovery
            .status,
        D::Fresh
    );
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 2);
    assert_eq!(
        f.database
            .search_catalog("once", f.now())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn initial_catalog_prefix_external_order_and_cache_sources() {
    use crate::models::search::Source;
    let f = Fixture::new().await;
    f.database.import_starters(r#"[{"video_id":"aaaaaa","title":"music","artist":"Artist"},{"video_id":"bbbbbb","title":"music","artist":"Artist"}]"#,NOW).await.unwrap();
    let body = serde_json::json!({"items":[
        {"id":{"kind":"youtube#video","videoId":"bbbbbb"},"snippet":{"title":"External duplicate","channelTitle":"Channel"}},
        {"id":{"kind":"youtube#video","videoId":"dddddd"},"snippet":{"title":"External first","channelTitle":"Channel"}},
        {"id":{"kind":"youtube#video","videoId":"cccccc"},"snippet":{"title":"External second","channelTitle":"Channel"}}
    ]});
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 90).await;
    let first = state
        .search
        .search(&f.database, "music", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        first
            .results
            .iter()
            .map(|r| r.song.video_id.as_str())
            .collect::<Vec<_>>(),
        ["aaaaaa", "bbbbbb", "dddddd", "cccccc"]
    );
    assert!(matches!(first.results[0].source, Source::Catalog));
    assert!(matches!(first.results[2].source, Source::Youtube));
    let cached = state
        .search
        .search(&f.database, "music", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        cached
            .results
            .iter()
            .map(|r| r.song.video_id.as_str())
            .collect::<Vec<_>>(),
        ["aaaaaa", "bbbbbb", "dddddd", "cccccc"]
    );
    assert!(matches!(cached.results[1].source, Source::YoutubeCache));
    assert_eq!(cached.results[1].song.title, "External duplicate");
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
    f.database.cleanup_catalog(NOW + 604800).await.unwrap();
    assert!(
        f.database
            .cached_search("music", NOW + 604800)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.database
            .search_catalog("external", NOW + 604800)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sum(submission_count) FROM catalog_songs")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn overlapping_errors_share_terminal_outcome_without_serial_retries() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_delay(std::time::Duration::from_millis(150)))
        .expect(1)
        .mount(&f.upstream)
        .await;
    let state = configured(&f, 90).await;
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let state = state.clone();
        jobs.spawn(async move {
            state
                .search
                .search(&state.database, "same", true, NOW)
                .await
                .unwrap()
        });
    }
    while let Some(r) = jobs.join_next().await {
        assert_eq!(
            r.unwrap().discovery.status,
            crate::models::search::DiscoveryStatus::TemporarilyUnavailable
        );
    }
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
    assert!(state.search.flights.lock().unwrap().is_empty());
}
