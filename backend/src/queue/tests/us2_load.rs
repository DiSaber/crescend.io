use super::*;
use reqwest::{Client, Method};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Instant,
};

type Samples = Arc<Mutex<Vec<(i64, Instant)>>>;
type SharedViews = Arc<Mutex<HashMap<i64, Value>>>;
struct Observer {
    account: usize,
    generation: String,
    first: i64,
    last: Option<i64>,
    samples: Samples,
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

fn validate(queue: &Value, shared: &SharedViews) -> i64 {
    let revision = queue["revision"].as_str().unwrap().parse().unwrap();
    let mut canonical = queue.clone();
    canonical.as_object_mut().unwrap().remove("membership_id");
    let mut ids = HashSet::new();
    for entry in queue["current"]
        .as_object()
        .map(|_| &queue["current"])
        .into_iter()
        .chain(queue["upcoming"].as_array().unwrap())
    {
        assert!(crate::models::queue::valid_id(
            entry["id"].as_str().unwrap()
        ));
        assert!(ids.insert(entry["id"].as_str().unwrap()));
        assert!(
            entry["submitted_by_user_id"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .is_ok()
        );
        for field in [
            "video_id",
            "title",
            "author_name",
            "author_url",
            "thumbnail_url",
            "embed_url",
        ] {
            assert!(entry["video"][field].is_string());
        }
        assert!(entry["active_vote_count"].as_i64().unwrap() >= 0);
        assert!(entry["has_voted"].is_boolean());
    }
    if queue["current"].is_null() {
        assert!(queue["upcoming"].as_array().unwrap().is_empty());
    }
    if let Some(current) = canonical["current"].as_object_mut() {
        current.remove("has_voted");
    }
    for entry in canonical["upcoming"].as_array_mut().unwrap() {
        entry.as_object_mut().unwrap().remove("has_voted");
    }
    let mut shared = shared.lock().unwrap();
    if let Some(prior) = shared.get(&revision) {
        assert_eq!(
            prior, &canonical,
            "shared fields disagree at revision {revision}"
        );
    } else {
        shared.insert(revision, canonical);
    }
    revision
}

async fn observe(
    client: &Client,
    base: &str,
    account: usize,
    generation: String,
    token: String,
    shared: SharedViews,
) -> Observer {
    let mut stream = client
        .get(format!(
            "{base}/api/lobbies/memberships/{generation}/events"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    assert!(
        super::us2_events::event(&mut stream)
            .await
            .contains("sync_required")
    );
    let url = format!("{base}/api/lobbies/memberships/{generation}/queue");
    let initial: Value = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let first = validate(&initial, &shared);
    let samples: Samples = Arc::new(Mutex::new(vec![(first, Instant::now())]));
    let samples_task = samples.clone();
    let (stop, mut stopping) = tokio::sync::watch::channel(false);
    let client = client.clone();
    let task = tokio::spawn(async move {
        let mut buffer = String::new();
        loop {
            tokio::select! {
                _ = stopping.changed() => break,
                chunk = stream.chunk() => {
                    let Some(chunk) = chunk.unwrap() else { assert_eq!(account, 20, "continuous observer disconnected"); break; };
                    buffer.push_str(&String::from_utf8(chunk.to_vec()).unwrap());
                    let mut dirty = false; let mut ended = false;
                    while let Some(end) = buffer.find("\n\n") {
                        let frame = buffer.drain(..end+2).collect::<String>();
                        dirty |= frame.contains("event: lobby_changed") || frame.contains("event: sync_required");
                        ended |= frame.contains("event: membership_ended");
                        assert!(!frame.contains("event: auth_expired"));
                    }
                    if ended { assert_eq!(account, 20); break; }
                    if dirty {
                        let response = client.get(&url).bearer_auth(&token).send().await.unwrap();
                        if response.status() == 403 && account == 20 { break; }
                        assert_eq!(response.status(), 200);
                        let queue: Value = response.json().await.unwrap();
                        let revision = validate(&queue, &shared);
                        samples_task.lock().unwrap().push((revision, Instant::now()));
                    }
                }
            }
        }
    });
    Observer {
        account,
        generation,
        first,
        last: None,
        samples,
        stop,
        task,
    }
}

#[tokio::test]
#[ignore = "explicit 120-second, 21-observer acceptance workload"]
async fn append_profile() {
    profile(false).await;
}
#[tokio::test]
#[ignore = "explicit 120-second, 21-observer acceptance workload"]
async fn mixed_profile() {
    profile(true).await;
}

async fn profile(mixed: bool) {
    let f = Arc::new(Fixture::new().await);
    let (host, membership, code) = f.host().await;
    let mut members = vec![(host, membership)];
    for i in 1..=20 {
        members.push(f.guest(&format!("guest-{i}"), &code).await);
    }
    f.mock_metadata().await;
    let ids = f.seed(&members[0].0, &members[0].1, 100).await;
    let reserved: HashSet<String> = ids[88..].iter().map(|id| id.as_str().into()).collect();
    if mixed {
        for id in &ids[88..] {
            for (guest, generation) in &members[1..=10] {
                f.database
                    .moderate_queue(
                        guest,
                        generation,
                        crate::database::queue::Moderation::Vote(id.as_str()),
                        || f.now(),
                    )
                    .await
                    .unwrap();
            }
        }
    }
    let initial = f
        .database
        .queue_view(&members[0].0, &members[0].1, || f.now())
        .await
        .unwrap();
    let initial_revision = initial.revision.value();
    let lobby = initial.lobby_id;
    let sqlite: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    println!(
        "profile={} SQLite={sqlite} logical_cores={} storage={} build=debug WAL pool=8",
        if mixed { "mixed" } else { "append" },
        std::thread::available_parallelism().unwrap(),
        f.directory.path().display()
    );
    let http = f.server().await;
    let client = Client::new();
    let tokens: Vec<String> = members
        .iter()
        .map(|(user, _)| f.auth.jwt.issue(user.id).unwrap())
        .collect();
    let shared: SharedViews = Default::default();
    let mut observations = Vec::new();
    for (account, (_, generation)) in members.iter().enumerate() {
        observations.push(
            observe(
                &client,
                &http.base,
                account,
                generation.clone(),
                tokens[account].clone(),
                shared.clone(),
            )
            .await,
        );
    }
    let observations = Arc::new(Mutex::new(observations));
    let members = Arc::new(members);
    let tokens = Arc::new(tokens);
    let latest_entry: Arc<Mutex<Option<String>>> = Default::default();
    let guest_generation = Arc::new(Mutex::new(members[20].1.clone()));
    let reserved = Arc::new(reserved);
    let logs: Arc<Mutex<Vec<(u64, String, i64, usize, String)>>> = Default::default();
    let schedule = tokio::time::Instant::now();
    let completed = Arc::new((0..120).map(|_| AtomicBool::new(false)).collect::<Vec<_>>());
    let mut mutations = Vec::new();
    for i in 0..120_u64 {
        let (
            f,
            client,
            base,
            members,
            tokens,
            latest_entry,
            guest_generation,
            observations,
            shared,
            reserved,
            logs,
            code,
        ) = (
            f.clone(),
            client.clone(),
            http.base.clone(),
            members.clone(),
            tokens.clone(),
            latest_entry.clone(),
            guest_generation.clone(),
            observations.clone(),
            shared.clone(),
            reserved.clone(),
            logs.clone(),
            code.clone(),
        );
        let completed = completed.clone();
        mutations.push(tokio::spawn(async move {
            tokio::time::sleep_until(schedule + Duration::from_secs(i)).await;
            assert!(tokio::time::Instant::now().duration_since(schedule + Duration::from_secs(i)) < Duration::from_millis(500), "missed operation {}", i+1);
            let queue_url = format!("{base}/api/lobbies/memberships/{}/queue", members[0].1);
            let operation = if mixed { i%10 } else { 0 };
            if mixed && !matches!(operation,0|5) {
                assert!(completed[i as usize-1].load(Ordering::SeqCst),"required predecessor missed operation {}",i+1);
            }
            let before:Value=if matches!(operation,2|3|4|7) {client.get(&queue_url).bearer_auth(&tokens[0]).send().await.unwrap().json().await.unwrap()} else {Value::Null};
            let (method, url, body, actor, kind) = match operation {
                0|9 => (Method::POST, format!("{queue_url}/entries"), Some(json!({"url":"https://youtu.be/dQw4w9WgXcQ","request_id":format!("{:032x}",1000+i)})), 0, "append"),
                1 => { let entry = latest_entry.lock().unwrap().clone().unwrap(); (Method::POST, format!("{base}/api/lobbies/memberships/{}/queue/entries/{entry}/votes", members[1].1), None, 1, "vote") },
                2 => { let entry = latest_entry.lock().unwrap().clone().unwrap(); (Method::POST, format!("{queue_url}/entries/{entry}/move"), Some(json!({"expected_revision":before["revision"],"to_index":0})), 0, "move") },
                3 => (Method::POST, format!("{queue_url}/advance"), Some(json!({"expected_entry_id":before["current"]["id"],"reason":"ended"})), 0, "advance"),
                4 => { let entry = before["upcoming"].as_array().unwrap().iter().find(|entry| !reserved.contains(entry["id"].as_str().unwrap())).unwrap()["id"].as_str().unwrap(); (Method::DELETE, format!("{queue_url}/entries/{entry}"), None, 0, "veto") },
                5 => (Method::PUT, format!("{queue_url}/voting-policy"), Some(json!({"mode":"fixed","count":20})), 0, "fixed"),
                6 => (Method::PUT, format!("{queue_url}/voting-policy"), Some(json!({"mode":"majority"})), 0, "majority"),
                7 => { let generation = guest_generation.lock().unwrap().clone(); (Method::DELETE, format!("{base}/api/lobbies/memberships/{generation}"), None, 20, "leave") },
                8 => (Method::POST, format!("{base}/api/lobbies/join"), Some(json!({"join_code":code})), 20, "join"),
                _ => unreachable!(),
            };
            if i == 59 { f.updates.suppress_next(); }
            let mut request = client.request(method, url).bearer_auth(&tokens[actor]); if let Some(body) = body { request = request.json(&body); }
            let response = request.send().await.unwrap(); assert_eq!(response.status(), if operation == 7 {204} else {200}, "operation {} {kind}", i+1);
            let result = if operation == 7 {Value::Null} else {response.json::<Value>().await.unwrap()};
            if !matches!(operation, 7|8) { assert_eq!(result["changed"], true, "operation {} {kind}", i+1); }
            if operation == 0 { *latest_entry.lock().unwrap() = Some(result["entry_id"].as_str().unwrap().into()); }
            let after: Value = if matches!(operation,7|8) {client.get(&queue_url).bearer_auth(&tokens[0]).send().await.unwrap().json().await.unwrap()} else {result["queue"].clone()};
            let revision = validate(&after, &shared);
            if operation==7 {assert_eq!(revision,before["revision"].as_str().unwrap().parse::<i64>().unwrap()+1);}
            if operation==8 {assert_eq!(revision,result["lobby"]["revision"].as_str().unwrap().parse::<i64>().unwrap());}
            if operation == 7 {
                let mut observers = observations.lock().unwrap(); let old = observers.iter_mut().rev().find(|observer| observer.account == 20).unwrap();
                old.last = Some(revision-1); let _ = old.stop.send(true);
                if i == 7 { assert!(after["upcoming"].as_array().unwrap().iter().all(|entry| !reserved.contains(entry["id"].as_str().unwrap()))); }
            }
            if operation == 8 {
                let generation = result["membership_id"].as_str().unwrap().to_owned(); *guest_generation.lock().unwrap() = generation.clone();
                let observer = observe(&client, &base, 20, generation, tokens[20].clone(), shared.clone()).await; observations.lock().unwrap().push(observer);
            }
            logs.lock().unwrap().push((i+1, kind.into(), revision, after["upcoming"].as_array().unwrap().len()+1,guest_generation.lock().unwrap().clone()));
            completed[i as usize].store(true,Ordering::SeqCst);
        }));
    }
    for task in mutations {
        task.await.unwrap();
    }
    let final_revision = initial_revision + 120;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if observations.lock().unwrap().iter().all(|observer| {
                observer
                    .samples
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(revision, _)| *revision >= observer.last.unwrap_or(final_revision))
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("all observer generations drain within 10 seconds");
    let observations = std::mem::take(&mut *observations.lock().unwrap());
    let mut finished = Vec::new();
    for observer in observations {
        let _ = observer.stop.send(true);
        observer.task.await.unwrap();
        finished.push((
            observer.account,
            observer.generation,
            observer.first,
            observer.last,
            observer.samples,
        ));
    }
    let commits = crate::queue::commits().lock().unwrap();
    let mut latencies = Vec::new();
    let mut artifact = String::from("revision,account,generation,latency_seconds\n");
    for revision in initial_revision + 1..=final_revision {
        let committed = commits[&(lobby.clone(), revision)];
        let mut required = 0;
        for (account, generation, first, last, samples) in &finished {
            if revision <= *first || revision > last.unwrap_or(final_revision) {
                continue;
            }
            required += 1;
            let observed = samples
                .lock()
                .unwrap()
                .iter()
                .find(|(seen, _)| *seen >= revision)
                .unwrap()
                .1;
            let elapsed = observed.duration_since(committed).as_secs_f64();
            assert!(
                elapsed <= 10.0,
                "revision {revision}, account {account}: {elapsed}s"
            );
            latencies.push(elapsed);
            artifact.push_str(&format!("{revision},{account},{generation},{elapsed:.9}\n"));
        }
        assert_eq!(
            required,
            if mixed && matches!((revision - initial_revision - 1) % 10, 7 | 8) {
                20
            } else {
                21
            }
        );
    }
    drop(commits);
    latencies.sort_by(f64::total_cmp);
    let name = if mixed { "mixed" } else { "append" };
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::write(output.join(format!("queue-{name}-samples.csv")), artifact).unwrap();
    let mut operations = logs.lock().unwrap().clone();
    operations.sort_by_key(|op| op.0);
    let log = String::from("operation,kind,revision,queue_length,guest20_generation\n")
        + &operations
            .into_iter()
            .map(|(op, kind, revision, len, generation)| {
                format!("{op},{kind},{revision},{len},{generation}\n")
            })
            .collect::<String>();
    std::fs::write(output.join(format!("queue-{name}-operations.csv")), log).unwrap();
    println!(
        "{name} samples={} max={:.6}s p50={:.6}s p95={:.6}s",
        latencies.len(),
        latencies.last().unwrap(),
        latencies[latencies.len() / 2],
        latencies[latencies.len() * 95 / 100]
    );
    let final_queue = f
        .database
        .queue_view(&members[0].0, &members[0].1, || f.now())
        .await
        .unwrap();
    assert_eq!(final_queue.upcoming.len() + 1, if mixed { 88 } else { 220 });
    assert_eq!(final_queue.voting.active_guest_count, 20);
}
