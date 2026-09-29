use super::us4_events::{connect, frame};
use super::*;
use futures_util::StreamExt;
use serde_json::Value;

// Fetch-equivalent parser: buffer bytes, not lossy per-chunk UTF-8 strings.
// CRLF, split UTF-8, multiple events and comment-only frames are supported.
#[derive(Default)]
struct Parser {
    bytes: Vec<u8>,
}
impl Parser {
    fn push(&mut self, chunk: &[u8]) -> Vec<(String, Value)> {
        self.bytes.extend_from_slice(chunk);
        assert!(self.bytes.len() <= 65536, "bounded parser buffer");
        let mut result = vec![];
        loop {
            let lf = self
                .bytes
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|p| (p, 2));
            let crlf = self
                .bytes
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| (p, 4));
            let Some((end, delimiter)) = lf.into_iter().chain(crlf).min() else {
                break;
            };
            let bytes: Vec<_> = self.bytes.drain(..end + delimiter).collect();
            let text = std::str::from_utf8(&bytes).unwrap();
            let mut event = "message";
            let mut data = Vec::new();
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    event = value.strip_prefix(' ').unwrap_or(value);
                }
                if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.strip_prefix(' ').unwrap_or(value));
                }
            }
            if !data.is_empty() {
                result.push((
                    event.to_owned(),
                    serde_json::from_str(&data.join("\n")).unwrap(),
                ));
            }
        }
        result
    }
}

#[derive(Default)]
struct Consumer {
    generation: String,
    epoch: u64,
    applied: i64,
    hinted: i64,
    dirty: bool,
    reading: bool,
    ended: bool,
    view: Option<Value>,
}
impl Consumer {
    fn bind(&mut self, generation: &str) {
        self.epoch += 1;
        self.generation = generation.into();
        self.applied = 0;
        self.hinted = 0;
        self.dirty = true;
        self.reading = false;
        self.ended = false;
        self.view = None;
    }
    fn event(&mut self, epoch: u64, name: &str, data: &Value) {
        if epoch != self.epoch || self.ended {
            return;
        }
        match name {
            "sync_required" => {
                self.dirty = true;
            }
            "lobby_changed" => {
                let revision = data["revision"].as_str().unwrap().parse::<i64>().unwrap();
                if revision > self.hinted.max(self.applied) {
                    self.hinted = revision;
                    self.dirty = true;
                }
            }
            "membership_ended" | "auth_expired" => {
                self.ended = true;
                self.view = None;
                self.dirty = false;
            }
            _ => {}
        }
    }
    fn start_read(&mut self) -> Option<u64> {
        if self.ended || self.reading || !self.dirty {
            return None;
        }
        self.dirty = false;
        self.reading = true;
        Some(self.epoch)
    }
    fn finish_read(&mut self, epoch: u64, view: Value) {
        if epoch != self.epoch || self.ended {
            return;
        }
        self.reading = false;
        if view["membership_id"].is_null() {
            self.ended = true;
            self.view = None;
            return;
        }
        if view["membership_id"] != self.generation {
            // Supersede the connection; its old in-flight responses are invalid.
            self.bind(view["membership_id"].as_str().unwrap());
        }
        let revision = view["lobby"]["revision"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap();
        if revision >= self.applied {
            self.applied = revision;
            self.view = Some(view);
        }
    }
}

async fn current(f: &Fixture) -> Value {
    f.client
        .get(format!("{}/api/lobbies/current", f.base))
        .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn local_connection(f: &Fixture, id: &str) -> crate::lobbies::updates::Connection {
    let receiver = f.updates.subscribe();
    let bound = f
        .database
        .stream_membership(f.user, id)
        .await
        .unwrap()
        .unwrap();
    crate::lobbies::updates::Connection::new(
        AppState {
            database: f.database.clone(),
            auth: f.auth.clone(),
            lobby_updates: f.updates.clone(),
        },
        f.auth
            .jwt
            .verify(&f.auth.jwt.issue(f.user).unwrap())
            .unwrap(),
        id.into(),
        bound,
        receiver,
    )
}

pub(super) async fn event_text(event: axum::response::sse::Event) -> String {
    use axum::response::IntoResponse;
    let body = axum::response::Sse::new(futures_util::stream::iter([Ok::<
        _,
        std::convert::Infallible,
    >(event)]))
    .into_response()
    .into_body();
    String::from_utf8(axum::body::to_bytes(body, 1024).await.unwrap().to_vec()).unwrap()
}

#[test]
fn split_framing_and_numeric_revision_filtering() {
    let source = ": café\r\n\r\nevent: sync_required\r\ndata: {\"revision\":\"9\"}\r\n\r\nevent: lobby_changed\ndata: {\"revision\":\"10\"}\n\n";
    for split in 0..source.len() {
        let mut parser = Parser::default();
        let mut events = parser.push(&source.as_bytes()[..split]);
        events.extend(parser.push(&source.as_bytes()[split..]));
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].1["revision"], "10");
    }
    let mut parser = Parser::default();
    assert_eq!(
        source
            .as_bytes()
            .iter()
            .flat_map(|b| parser.push(&[*b]))
            .count(),
        2
    );
}

#[tokio::test]
async fn setup_race_lag_duplicates_and_slow_consumer_use_authoritative_revision() {
    let f = Fixture::new().await;
    let view: Value = f.create().await.json().await.unwrap();
    let id = view["membership_id"].as_str().unwrap();
    // Subscribe and authorize, then commit before the first stream poll.
    let connection = local_connection(&f, id).await;
    let other = f.database.create_user("setup-race").await.unwrap().id;
    let joined = f
        .database
        .join_lobby(other, view["lobby"]["join_code"].as_str().unwrap(), || NOW)
        .await
        .unwrap();
    let lobby = joined.lobby.unwrap();
    for _ in 0..1024 {
        f.updates.publish(lobby.id.clone(), 999);
    }
    let mut stream = Box::pin(connection.stream());
    let initial = event_text(stream.next().await.unwrap().unwrap()).await;
    assert!(initial.contains("sync_required") && initial.contains("\"revision\":\"2\""));
    // Hints never dictate the revision, including forged, stale, or duplicate hints.
    let third = f.database.create_user("slow-reader").await.unwrap().id;
    f.database
        .join_lobby(third, &lobby.join_code, || NOW)
        .await
        .unwrap();
    for _ in 0..1024 {
        f.updates.publish(lobby.id.clone(), 999);
    }
    let changed = event_text(
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .await;
    assert!(changed.contains("lobby_changed") && changed.contains("\"revision\":\"3\""));
    assert!(
        tokio::time::timeout(Duration::from_millis(1100), stream.next())
            .await
            .is_err()
    );
    assert_eq!(current(&f).await["lobby"]["revision"], "3");
}

#[tokio::test]
async fn ended_generation_storage_failure_and_queued_hints_never_emit_protected_updates() {
    for mode in ["removed", "closed", "storage", "auth", "expiry"] {
        let f = Fixture::new().await;
        let view: Value = f.create().await.json().await.unwrap();
        let id = view["membership_id"].as_str().unwrap();
        if mode == "expiry" {
            f.time.store(NOW + 86_399, Ordering::SeqCst);
        }
        let connection = local_connection(&f, id).await;
        let mut stream = Box::pin(connection.stream());
        let _ = stream.next().await.unwrap().unwrap();
        let lobby_id = f
            .database
            .stream_membership(f.user, id)
            .await
            .unwrap()
            .unwrap()
            .lobby_id;
        f.updates.publish(lobby_id, 999);
        match mode {
            "removed" => {
                sqlx::query("DELETE FROM lobby_memberships")
                    .execute(&f.database.db_pool)
                    .await
                    .unwrap();
            }
            "closed" => {
                sqlx::query("UPDATE lobbies SET closed_at = created_at")
                    .execute(&f.database.db_pool)
                    .await
                    .unwrap();
            }
            "storage" => {
                f.database.db_pool.close().await;
            }
            "auth" => {
                f.time.store(NOW + 3600, Ordering::SeqCst);
            }
            _ => {
                f.time.store(NOW + 86_400, Ordering::SeqCst);
            }
        }
        let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap();
        if mode != "storage" {
            let terminal = event_text(next.unwrap().unwrap()).await;
            assert!(terminal.contains(if mode == "auth" {
                "auth_expired"
            } else if mode == "expiry" {
                "\"reason\":\"expired\""
            } else if mode == "closed" {
                "\"reason\":\"closed\""
            } else {
                "\"reason\":\"left\""
            }));
            assert!(stream.next().await.is_none());
        } else {
            assert!(next.is_none());
        }
    }
}

#[tokio::test]
async fn blocked_storage_does_not_postpone_auth_expiry() {
    let f = Fixture::new().await;
    let view: Value = f.create().await.json().await.unwrap();
    let mut stream = connect(&f, view["membership_id"].as_str().unwrap(), f.user).await;
    frame(&mut stream).await;
    let mut connections = Vec::new();
    for _ in 0..4 {
        connections.push(f.database.db_pool.acquire().await.unwrap());
    }
    f.time.store(NOW + 3600, Ordering::SeqCst);
    let start = std::time::Instant::now();
    assert!(frame(&mut stream).await.contains("auth_expired"));
    assert!(start.elapsed() < Duration::from_millis(1500));
    assert!(stream.chunk().await.unwrap().is_none());
}

#[tokio::test]
async fn representative_client_coalesces_refetch_and_rejects_obsolete_generations() {
    let f = Fixture::new().await;
    let view: Value = f.create().await.json().await.unwrap();
    let id = view["membership_id"].as_str().unwrap();
    let mut consumer = Consumer::default();
    consumer.bind(id);
    let mut stream = connect(&f, id, f.user).await;
    let mut parser = Parser::default();
    for (name, data) in parser.push(frame(&mut stream).await.as_bytes()) {
        consumer.event(consumer.epoch, &name, &data);
    }
    let ticket = consumer.start_read().unwrap();
    let stale = current(&f).await;
    let other = f
        .database
        .create_user("invalidation-during-read")
        .await
        .unwrap()
        .id;
    f.database
        .join_lobby(other, view["lobby"]["join_code"].as_str().unwrap(), || NOW)
        .await
        .unwrap();
    for (name, data) in parser.push(frame(&mut stream).await.as_bytes()) {
        consumer.event(ticket, &name, &data);
        consumer.event(ticket, &name, &data);
    }
    assert!(consumer.start_read().is_none());
    consumer.finish_read(ticket, stale.clone());
    let next = consumer
        .start_read()
        .expect("invalidation during read requires another read");
    consumer.finish_read(next, current(&f).await);
    assert_eq!(consumer.applied, 2);
    consumer.finish_read(next, stale.clone());
    assert_eq!(consumer.applied, 2);
    drop(stream);
    let started = std::time::Instant::now();
    let mut stream = connect(&f, id, f.user).await;
    for (name, data) in parser.push(frame(&mut stream).await.as_bytes()) {
        consumer.event(ticket, &name, &data);
    }
    let next = consumer.start_read().unwrap();
    consumer.finish_read(next, current(&f).await);
    assert!(started.elapsed() < Duration::from_secs(3));
    // Seed a replacement generation without implementing US5.
    sqlx::query("DELETE FROM lobby_memberships WHERE user_id = ?")
        .bind(f.user)
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let replacement = f
        .database
        .join_lobby(f.user, view["lobby"]["join_code"].as_str().unwrap(), || NOW)
        .await
        .unwrap();
    consumer.bind(replacement.membership_id.unwrap().as_str());
    consumer.finish_read(ticket, stale);
    consumer.event(ticket, "auth_expired", &serde_json::json!({}));
    assert!(!consumer.ended);
    assert!(consumer.view.is_none());
    let next = consumer.start_read().unwrap();
    consumer.finish_read(next, current(&f).await);
    assert_eq!(consumer.applied, 3);
    let epoch = consumer.epoch;
    consumer.event(
        epoch,
        "membership_ended",
        &serde_json::json!({"reason":"expired"}),
    );
    consumer.finish_read(epoch, current(&f).await);
    assert!(consumer.view.is_none());
}

#[tokio::test]
async fn committed_join_without_publication_is_recovered() {
    let f = Fixture::new().await;
    let view: Value = f.create().await.json().await.unwrap();
    let mut stream = connect(&f, view["membership_id"].as_str().unwrap(), f.user).await;
    frame(&mut stream).await;
    let other = f.database.create_user("lost-publication").await.unwrap().id;
    // Direct commit deliberately skips the route's publication, including the cancellation gap.
    f.database
        .join_lobby(other, view["lobby"]["join_code"].as_str().unwrap(), || NOW)
        .await
        .unwrap();
    assert!(frame(&mut stream).await.contains("\"revision\":\"2\""));
    let recovered: Value = f
        .client
        .get(format!("{}/api/lobbies/current", f.base))
        .bearer_auth(f.auth.jwt.issue(f.user).unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["lobby"]["members"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit 200-connection performance validation"]
async fn baseline_200_connections_converges_and_recovers() {
    use futures_util::future::join_all;
    use std::time::Instant;
    struct Group {
        code: String,
        lobby: String,
        clients: Vec<(i64, String, Response)>,
    }
    let f = Fixture::new().await;
    let mut groups = Vec::new();
    for group in 0..20 {
        let owner = f
            .database
            .create_user(&format!("load-{group}-owner"))
            .await
            .unwrap()
            .id;
        let view = f.database.create_owned_lobby(owner, || NOW).await.unwrap();
        let lobby = view.lobby.unwrap();
        let mut clients = vec![(owner, view.membership_id.unwrap().as_str().to_owned())];
        for member in 1..10 {
            let user = f
                .database
                .create_user(&format!("load-{group}-{member}"))
                .await
                .unwrap()
                .id;
            let view = f
                .database
                .join_lobby(user, &lobby.join_code, || NOW)
                .await
                .unwrap();
            clients.push((user, view.membership_id.unwrap().as_str().to_owned()));
        }
        let clients = join_all(clients.into_iter().map(|(user, id)| {
            let f = &f;
            async move {
                let mut stream = connect(f, &id, user).await;
                frame(&mut stream).await;
                (user, id, stream)
            }
        }))
        .await;
        groups.push(Group {
            code: lobby.join_code,
            lobby: lobby.id.as_str().to_owned(),
            clients,
        });
    }
    let mut actions = vec![];
    let mut updates = vec![];
    let mut fallback = vec![];
    for round in 0..10 {
        let mut joiners = vec![];
        for group in 0..20 {
            joiners.push(
                f.database
                    .create_user(&format!("load-extra-{round}-{group}"))
                    .await
                    .unwrap()
                    .id,
            );
        }
        let results = join_all(groups.iter_mut().zip(joiners).map(|(group, user)| {
            let f = &f;
            async move {
                let start = Instant::now();
                if round < 5 {
                    let response = f
                        .client
                        .post(format!("{}/api/lobbies/join", f.base))
                        .bearer_auth(f.auth.jwt.issue(user).unwrap())
                        .json(&serde_json::json!({"join_code":group.code}))
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(response.status(), 200);
                    let _: Value = response.json().await.unwrap();
                } else {
                    f.database
                        .join_lobby(user, &group.code, || NOW)
                        .await
                        .unwrap();
                }
                let action = start.elapsed().as_secs_f64();
                let times = join_all(group.clients.iter_mut().map(|(user, _, stream)| {
                    let lobby = &group.lobby;
                    async move {
                        let hint = frame(stream).await;
                        assert!(hint.contains("lobby_changed"));
                        let view: Value = f
                            .client
                            .get(format!("{}/api/lobbies/current", f.base))
                            .bearer_auth(f.auth.jwt.issue(*user).unwrap())
                            .send()
                            .await
                            .unwrap()
                            .json()
                            .await
                            .unwrap();
                        assert_eq!(view["lobby"]["id"], *lobby);
                        assert_eq!(
                            view["lobby"]["members"].as_array().unwrap().len(),
                            11 + round
                        );
                        start.elapsed().as_secs_f64()
                    }
                }))
                .await;
                (action, times)
            }
        }))
        .await;
        for (action, times) in results {
            actions.push(action);
            if round < 5 {
                updates.extend(times);
            } else {
                fallback.extend(times);
            }
        }
    }
    let mut reconnect = vec![];
    for group in &mut groups {
        for (user, id, stream) in group.clients.iter_mut().take(5) {
            let start = Instant::now();
            // Drop the old body before establishing its replacement.
            let replacement = f
                .client
                .get(format!("{}/scalar", f.base))
                .send()
                .await
                .unwrap();
            drop(std::mem::replace(stream, replacement));
            *stream = connect(&f, id, *user).await;
            assert!(frame(stream).await.contains("sync_required"));
            let view: Value = f
                .client
                .get(format!("{}/api/lobbies/current", f.base))
                .bearer_auth(f.auth.jwt.issue(*user).unwrap())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(view["lobby"]["revision"], "20");
            reconnect.push(start.elapsed().as_secs_f64());
        }
    }
    fn p95(values: &mut [f64]) -> f64 {
        values.sort_by(f64::total_cmp);
        values[(values.len() * 95).div_ceil(100) - 1]
    }
    let action = p95(&mut actions);
    let update = p95(&mut updates);
    let missed = p95(&mut fallback);
    let recovery = p95(&mut reconnect);
    println!(
        "200 clients / 20 lobbies; actions={} p95={action:.4}s; update+refetch={} p95={update:.4}s; lost-publication+refetch={} p95={missed:.4}s; reconnect+refetch={} p95={recovery:.4}s",
        actions.len(),
        updates.len(),
        fallback.len(),
        reconnect.len()
    );
    assert!(action < 2.0 && update < 2.0 && missed < 2.0 && recovery < 3.0);
}
