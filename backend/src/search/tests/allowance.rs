use super::*;
#[tokio::test]
async fn atomic_limit_restart_and_utc_rollover() {
    let f = Fixture::new().await;
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let db = f.database.clone();
        jobs.spawn(async move { db.reserve_search(NOW, 1).await.unwrap() });
    }
    let mut accepted = 0;
    while let Some(r) = jobs.join_next().await {
        accepted += usize::from(r.unwrap());
    }
    assert_eq!(accepted, 1);
    let reopened = Fixture::connect(&f.directory.path().join("search.db")).await;
    assert!(!reopened.reserve_search(NOW, 1).await.unwrap());
    assert!(!reopened.reserve_search(NOW, 0).await.unwrap());
    assert!(
        reopened
            .reserve_search((NOW / 86400 + 1) * 86400, 1)
            .await
            .unwrap()
    );
    let used: i64 = sqlx::query_scalar("SELECT sum(attempts) FROM song_search_daily_usage")
        .fetch_one(&reopened.db_pool)
        .await
        .unwrap();
    assert_eq!(used, 2);
}
#[tokio::test]
async fn snapshots_are_exclusive_and_corruption_is_a_miss() {
    let f = Fixture::new().await;
    f.database
        .save_search("query", &[song("aaaaaa", "title", "artist")], NOW)
        .await
        .unwrap();
    assert!(
        f.database
            .cached_search("query", NOW + 604799)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        f.database
            .cached_search("query", NOW + 604800)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE song_search_cache SET results_json='{}'")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert!(
        f.database
            .cached_search("query", NOW)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("DROP TABLE song_search_cache")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    assert!(f.database.cached_search("query", NOW).await.is_err());
}

#[tokio::test]
async fn configured_zero_cap_and_reservation_failure_never_send() {
    use crate::models::search::DiscoveryStatus as D;
    let f = Fixture::new().await;
    let state = super::discovery::configured(&f, 0).await;
    assert_eq!(
        state
            .search
            .search(&f.database, "zero", true, NOW)
            .await
            .unwrap()
            .discovery
            .status,
        D::DailyLimitReached
    );
    sqlx::query("DROP TABLE song_search_daily_usage")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let state = super::discovery::configured(&f, 90).await;
    assert_eq!(
        state
            .search
            .search(&f.database, "broken", true, NOW)
            .await
            .unwrap()
            .discovery
            .status,
        D::TemporarilyUnavailable
    );
    assert!(f.upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_distinct_queries_share_final_slot_and_lock_failure_is_safe() {
    use crate::models::search::DiscoveryStatus as D;
    use wiremock::{Mock, ResponseTemplate, matchers::method};
    let f = Fixture::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&f.upstream)
        .await;
    let state = super::discovery::configured(&f, 1).await;
    let mut jobs = tokio::task::JoinSet::new();
    for i in 0..20 {
        let state = state.clone();
        jobs.spawn(async move {
            state
                .search
                .search(&state.database, &format!("distinct{i}"), true, NOW)
                .await
                .unwrap()
                .discovery
                .status
        });
    }
    let mut denied = 0;
    let mut failed = 0;
    while let Some(result) = jobs.join_next().await {
        match result.unwrap() {
            D::DailyLimitReached => denied += 1,
            D::TemporarilyUnavailable => failed += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((denied, failed), (19, 1));
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sum(attempts) FROM song_search_daily_usage")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        1
    );
    let state = super::discovery::configured(&f, 90).await;
    let lock = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    assert_eq!(
        state
            .search
            .search(&f.database, "locked", true, NOW)
            .await
            .unwrap()
            .discovery
            .status,
        D::TemporarilyUnavailable
    );
    lock.rollback().await.unwrap();
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
}
