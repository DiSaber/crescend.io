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
        .and(query_param("maxResults", "25"))
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

fn candidate(id: &str, title: &str) -> serde_json::Value {
    serde_json::json!({"id":{"kind":"youtube#video","videoId":id},"snippet":{"title":title,"channelTitle":"Channel"}})
}
fn details(id: &str, duration: &str, topic: &str) -> serde_json::Value {
    serde_json::json!({"id":id,"contentDetails":{"duration":duration},
        "topicDetails":{"topicCategories":[format!("https://en.wikipedia.org/wiki/{topic}")]}})
}
async fn filtered(f: &Fixture) -> AppState {
    let mut state = f.state();
    state.search = super::super::SearchService::new(
        super::super::config::SearchConfig {
            api_key: Some("private-test-key".into()),
            daily_limit: 90,
        },
        f.auth.clock.clone(),
        crate::youtube::search::SearchClient::for_filter_test(f.upstream.uri().parse().unwrap()),
    );
    state
}

#[tokio::test]
async fn discovery_keeps_only_music_of_a_queueable_length() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":[
                candidate("aaaaaa", "Real song"),
                candidate("bbbbbb", "Cat compilation"),
                candidate("cccccc", "Ten hour loop"),
                candidate("dddddd", "Music short"),
                candidate("eeeeee", "Deleted before lookup"),
                candidate("ffffff", "Genre-only topic"),
            ]})),
        )
        .expect(1)
        .mount(&f.upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .and(query_param("part", "contentDetails,topicDetails"))
        .and(query_param(
            "id",
            "aaaaaa,bbbbbb,cccccc,dddddd,eeeeee,ffffff",
        ))
        .and(header("x-goog-api-key", "private-test-key"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":[
                // Lookup order differs from search order, which must be preserved.
                details("ffffff", "PT3M", "Hip_hop_music"),
                details("aaaaaa", "PT4M13S", "Music"),
                details("bbbbbb", "PT2M43S", "Pet"),
                details("cccccc", "PT10H", "Music"),
                details("dddddd", "PT45S", "Music"),
            ]})),
        )
        .expect(1)
        .mount(&f.upstream)
        .await;
    let state = filtered(&f).await;
    let found = state
        .search
        .search(&f.database, "anything", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        found.discovery.status,
        crate::models::search::DiscoveryStatus::Fresh
    );
    let ids: Vec<_> = found
        .results
        .iter()
        .map(|r| r.song.video_id.as_str())
        .collect();
    assert_eq!(ids, ["aaaaaa", "ffffff"]);
    // Rejected videos never reach the shared catalog.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let saved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM catalog_songs WHERE video_id IN ('bbbbbb','cccccc','dddddd','eeeeee')")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(saved, 0);
}

#[tokio::test]
async fn failed_music_lookup_returns_and_saves_nothing_external() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"items":[candidate("aaaaaa", "Unchecked song")]}),
            ),
        )
        .mount(&f.upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&f.upstream)
        .await;
    let state = filtered(&f).await;
    let found = state
        .search
        .search(&f.database, "unchecked", true, NOW)
        .await
        .unwrap();
    assert_eq!(
        found.discovery.status,
        crate::models::search::DiscoveryStatus::TemporarilyUnavailable
    );
    assert!(found.results.is_empty());
    let saved: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM catalog_songs WHERE video_id = 'aaaaaa'")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
    assert_eq!(saved, 0);
}

#[tokio::test]
async fn empty_search_skips_the_music_lookup_and_results_stop_at_ten() {
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("q", "nothing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":[]})))
        .mount(&f.upstream)
        .await;
    let ids: Vec<String> = (0..25).map(|n| format!("vid{n:03}")).collect();
    let page: Vec<_> = ids.iter().map(|id| candidate(id, "Song")).collect();
    let info: Vec<_> = ids
        .iter()
        .map(|id| details(id, "PT3M", "Pop_music"))
        .collect();
    Mock::given(method("GET"))
        .and(path("/"))
        .and(query_param("q", "many"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":page})))
        .mount(&f.upstream)
        .await;
    // One lookup in total: the "many" search only.
    Mock::given(method("GET"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":info})))
        .expect(1)
        .mount(&f.upstream)
        .await;
    let state = filtered(&f).await;
    let empty = state
        .search
        .search(&f.database, "nothing", true, NOW)
        .await
        .unwrap();
    assert!(empty.results.is_empty());
    let many = state
        .search
        .search(&f.database, "many", true, NOW)
        .await
        .unwrap();
    assert_eq!(many.results.len(), 10);
}

#[tokio::test]
async fn official_result_past_the_tenth_candidate_is_kept() {
    let f = Fixture::new().await;
    let ids: Vec<String> = (0..25).map(|n| format!("vid{n:03}")).collect();
    let mut page: Vec<_> = ids[..24]
        .iter()
        .map(|id| candidate(id, "Fan upload"))
        .collect();
    page.push(
        serde_json::json!({"id":{"kind":"youtube#video","videoId":"vid024"},
        "snippet":{"title":"Song (Official Video)","channelTitle":"ArtistVEVO"}}),
    );
    let info: Vec<_> = ids.iter().map(|id| details(id, "PT3M", "Music")).collect();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":page})))
        .mount(&f.upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":info})))
        .mount(&f.upstream)
        .await;
    let state = filtered(&f).await;
    let found = state
        .search
        .search(&f.database, "song", true, NOW)
        .await
        .unwrap();
    assert_eq!(found.results.len(), 10);
    assert_eq!(found.results[0].song.video_id, "vid024");
}

#[test]
fn iso_durations_parse_or_reject() {
    use crate::youtube::search::iso_seconds;
    assert_eq!(iso_seconds("PT4M13S"), Some(253));
    assert_eq!(iso_seconds("PT1H"), Some(3600));
    assert_eq!(iso_seconds("P1DT2H"), Some(93600));
    assert_eq!(iso_seconds("P0D"), Some(0));
    for bad in ["", "P", "4M13S", "PT4X", "PT4", "PT1H2", "P1H", "PT1D"] {
        assert_eq!(iso_seconds(bad), None, "{bad}");
    }
}

#[tokio::test]
async fn official_youtube_results_rank_above_weaker_catalog_matches() {
    let f = Fixture::new().await;
    f.database
        .record_submission(
            "11111111111111111111111111111111",
            &song("bbbbbb", "Teen Spirit (Piano Cover)", "Someone"),
            NOW,
        )
        .await
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":[
            candidate("cccccc", "Teen Spirit (Lyrics)"),
            {"id":{"kind":"youtube#video","videoId":"dddddd"},
             "snippet":{"title":"Teen Spirit (Official Music Video)","channelTitle":"NirvanaVEVO"}},
        ]})))
        .mount(&f.upstream)
        .await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":[
                details("cccccc", "PT5M", "Music"),
                details("dddddd", "PT5M", "Rock_music"),
            ]})),
        )
        .mount(&f.upstream)
        .await;
    let state = filtered(&f).await;
    let found = state
        .search
        .search(&f.database, "teen spirit", true, NOW)
        .await
        .unwrap();
    let ids: Vec<_> = found
        .results
        .iter()
        .map(|r| r.song.video_id.as_str())
        .collect();
    assert_eq!(ids, ["dddddd", "cccccc", "bbbbbb"]);
    // The same order comes back from the saved search, with no further provider request.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let requests = f.upstream.received_requests().await.unwrap().len();
    let again = state
        .search
        .search(&f.database, "teen spirit", true, NOW)
        .await
        .unwrap();
    let ids: Vec<_> = again
        .results
        .iter()
        .map(|r| r.song.video_id.as_str())
        .collect();
    assert_eq!(ids[0], "dddddd");
    assert_eq!(
        f.upstream.received_requests().await.unwrap().len(),
        requests
    );
}
