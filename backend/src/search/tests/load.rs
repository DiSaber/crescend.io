use super::*;
use std::time::{Duration, Instant};
use wiremock::{Mock, ResponseTemplate, matchers::method};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_search_latency() {
    let f = Fixture::new().await;
    let rows:Vec<_>=(0..10000).map(|i| serde_json::json!({"video_id":format!("{i:011}"),"title":format!("group{:02} music {i}",i%20),"artist":"Artist","thumbnail_url":null})).collect();
    f.database
        .import_starters(&serde_json::to_string(&rows).unwrap(), NOW)
        .await
        .unwrap();
    let state = f.state();
    let server = f.server(state).await;
    let warm_user = f.account("warm").await;
    reqwest::Client::new()
        .get(format!("{}/api/songs/search?q=group00", server.base))
        .bearer_auth(f.auth.jwt.issue(warm_user.id).unwrap())
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let mut jobs = tokio::task::JoinSet::new();
    for account in 0..20 {
        let user = f.account(&format!("load{account}")).await;
        let token = f.auth.jwt.issue(user.id).unwrap();
        let base = server.base.clone();
        jobs.spawn(async move {
            let client = reqwest::Client::new();
            let mut samples = Vec::new();
            let mut failed = 0;
            for i in 0..50 {
                let start = Instant::now();
                let response = client
                    .get(format!(
                        "{base}/api/songs/search?q=group{:02}",
                        (i + account) % 20
                    ))
                    .bearer_auth(&token)
                    .send()
                    .await;
                let valid = match response {
                    Ok(r) if r.status() == 200 => {
                        r.json::<serde_json::Value>().await.is_ok_and(|v| {
                            v["discovery"]["status"] == "not_needed"
                                && v["results"].as_array().is_some_and(|r| r.len() == 10)
                        })
                    }
                    _ => false,
                };
                samples.push(start.elapsed());
                if !valid {
                    failed += 1;
                }
            }
            (samples, failed)
        });
    }
    let mut samples = Vec::new();
    let mut failures = 0;
    while let Some(r) = jobs.join_next().await {
        let (s, f) = r.unwrap();
        samples.extend(s);
        failures += f;
    }
    samples.sort();
    let p95 = samples[(samples.len() * 95).div_ceil(100) - 1];
    println!(
        "catalog_search_latency os={} arch={} profile=debug sqlite=file/WAL songs=10000 clients=20 queries=20 requests={} failures={} min={:?} median={:?} p95={p95:?} max={:?}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        samples.len(),
        failures,
        samples[0],
        samples[samples.len() / 2],
        samples.last().unwrap()
    );
    assert_eq!(samples.len(), 1000);
    assert_eq!(failures, 0);
    assert!(p95 < Duration::from_secs(1));
    assert!(f.upstream.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn degraded_discovery_latency() {
    let f = Fixture::new().await;
    f.database
        .import_starters(
            r#"[{"video_id":"aaaaaa","title":"known","artist":"artist"}]"#,
            NOW,
        )
        .await
        .unwrap();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(10))
                .set_body_json(serde_json::json!({"items":[]})),
        )
        .mount(&f.upstream)
        .await;
    let state = super::discovery::configured(&f, 90).await;
    let server = f.server(state.clone()).await;
    let user = f.account("degraded").await;
    let token = f.auth.jwt.issue(user.id).unwrap();
    let mut jobs = tokio::task::JoinSet::new();
    for i in 0..20 {
        let base = server.base.clone();
        let token = token.clone();
        jobs.spawn(async move {
            let start = Instant::now();
            // Repeated literal punctuation varies provider queries while retaining catalog matches.
            let q = format!("known{}", "!".repeat(i));
            let url = url::Url::parse_with_params(&format!("{base}/api/songs/search"), &[("q", q)])
                .unwrap();
            let response = reqwest::Client::new()
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let value: serde_json::Value = response.json().await.unwrap();
            assert_eq!(value["discovery"]["status"], "temporarily_unavailable");
            assert_eq!(value["results"][0]["video_id"], "aaaaaa");
            start.elapsed()
        });
    }
    let mut samples = Vec::new();
    while let Some(r) = jobs.join_next().await {
        let elapsed = r.unwrap();
        assert!(elapsed < Duration::from_secs(6));
        samples.push(elapsed);
    }
    samples.sort();
    println!(
        "degraded_discovery_latency os={} clients=20 distinct=20 failures=0 p95={:?} max={:?}",
        std::env::consts::OS,
        samples[18],
        samples.last().unwrap()
    );
    assert_eq!(f.upstream.received_requests().await.unwrap().len(), 20);
    assert!(state.search.flights.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_success_and_empty_survive_blocked_persistence() {
    for empty in [false, true] {
        let f = Fixture::new().await;
        let body = if empty {
            serde_json::json!({"items":[]})
        } else {
            super::discovery::provider_body()
        };
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(2800))
                    .set_body_json(body),
            )
            .expect(1)
            .mount(&f.upstream)
            .await;
        let mut state = super::discovery::configured(&f, 90).await;
        let service = Arc::get_mut(&mut state.search).unwrap();
        service.before_provider = Duration::from_millis(1550);
        service.block_persistence = true;
        let server = f.server(state.clone()).await;
        let user = f.account("late-client").await;
        let token = f.auth.jwt.issue(user.id).unwrap();
        let started = Instant::now();
        let mut jobs = tokio::task::JoinSet::new();
        for _ in 0..20 {
            let endpoint = format!("{}/api/songs/search?q=late&more=true", server.base);
            let token = token.clone();
            jobs.spawn(async move {
                let response = reqwest::Client::new()
                    .get(endpoint)
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                response.json::<serde_json::Value>().await.unwrap()
            });
        }
        while let Some(r) = jobs.join_next().await {
            let response = r.unwrap();
            assert_eq!(response["discovery"]["status"], "fresh");
            assert_eq!(
                response["results"].as_array().unwrap().len(),
                usize::from(!empty)
            );
            assert!(started.elapsed() < Duration::from_secs(6));
        }
        // The outcome is published before the blocked persistence deadline.
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(state.search.flights.lock().unwrap().is_empty());
        assert!(
            f.database
                .cached_search("late", NOW)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(f.upstream.received_requests().await.unwrap().len(), 1);
        println!(
            "late_success empty={empty} callers=20 failures=0 elapsed={:?}",
            started.elapsed()
        );
    }
}
