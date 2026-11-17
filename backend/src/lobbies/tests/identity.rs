//! Join and identity rules from QA (docs/guest-join.md): the host's own join link,
//! codes typed with spaces, wrong codes, host removal of members, presence, emoji
//! nicknames and the nickname table migration.
use super::us4_events::{connect, frame};
use super::*;
use serde_json::{Value, json};

async fn call(
    f: &Fixture,
    method: reqwest::Method,
    user: i64,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = f
        .client
        .request(method, format!("{}{path}", f.base))
        .bearer_auth(f.auth.jwt.issue(user).unwrap());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn post(f: &Fixture, user: i64, path: &str, body: Value) -> (u16, Value) {
    call(f, reqwest::Method::POST, user, path, Some(body)).await
}

async fn delete(f: &Fixture, user: i64, path: &str) -> (u16, Value) {
    call(f, reqwest::Method::DELETE, user, path, None).await
}

async fn get(f: &Fixture, user: i64, path: &str) -> Value {
    let (status, body) = call(f, reqwest::Method::GET, user, path, None).await;
    assert_eq!(status, 200, "{path}: {body}");
    body
}

fn names(view: &Value) -> Vec<String> {
    view["lobby"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["nickname"].as_str().unwrap().to_owned())
        .collect()
}

/// Host "Daniel" plus the named guests; returns (host view, [(guest id, membership)]).
async fn party(f: &Fixture, guests: &[&str]) -> (Value, Vec<(i64, String)>) {
    let (status, host) = post(f, f.user, "/api/lobbies", json!({"nickname": "Daniel"})).await;
    assert_eq!(status, 201, "{host}");
    let code = host["lobby"]["join_code"].as_str().unwrap().to_owned();
    let mut joined = Vec::new();
    for (i, name) in guests.iter().enumerate() {
        let id = f
            .database
            .create_user(&format!("guest-{i}"))
            .await
            .unwrap()
            .id;
        let (status, view) = post(
            f,
            id,
            "/api/lobbies/join",
            json!({"join_code": code, "nickname": name}),
        )
        .await;
        assert_eq!(status, 200, "{view}");
        joined.push((id, view["membership_id"].as_str().unwrap().to_owned()));
    }
    (host, joined)
}

async fn users(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap()
}

async fn guest_sign_in(f: &Fixture, query: &str) -> (u16, Value) {
    let response = f
        .client
        .post(format!("{}/api/auth/guest{query}", f.base))
        .header("X-CSRF-Protection", "1")
        .header("X-Crescend-Client-IP", "198.51.100.40")
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

#[tokio::test]
async fn the_host_opening_their_own_join_link_is_never_renamed() {
    let f = Fixture::new().await;
    let (host, _) = party(&f, &["Kat"]).await;
    let code = host["lobby"]["join_code"].as_str().unwrap();
    let before = get(&f, f.user, "/api/lobbies/current").await;
    let (status, view) = post(
        &f,
        f.user,
        "/api/lobbies/join",
        json!({"join_code": code, "nickname": "Danny"}),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(names(&view), ["Daniel", "Kat"]);
    assert_eq!(view["membership_id"], before["membership_id"]);
    assert_eq!(view["lobby"]["revision"], before["lobby"]["revision"]);
    // The account's last nickname is not touched either.
    assert_eq!(get(&f, f.user, "/api/me").await["last_nickname"], "Daniel");
}

#[tokio::test]
async fn codes_typed_with_spaces_or_dashes_join_and_bad_codes_say_so_plainly() {
    let f = Fixture::new().await;
    let (host, _) = party(&f, &[]).await;
    let code = host["lobby"]["join_code"].as_str().unwrap().to_owned();
    let spaced = format!(
        " {} - {} ",
        code[..3].to_lowercase(),
        code[3..].to_lowercase()
    );
    let guest = f.database.create_user("spaced").await.unwrap().id;
    let (status, view) = post(
        &f,
        guest,
        "/api/lobbies/join",
        json!({"join_code": spaced, "nickname": "Sam"}),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(names(&view), ["Daniel", "Sam"]);
    for bad in ["AB", "ABC1234", "ABC_12", "ÄBC123"] {
        let (status, body) = post(
            &f,
            guest,
            "/api/lobbies/join",
            json!({"join_code": bad, "nickname": "Sam"}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("invalid_join_code")),
            "{bad}"
        );
        assert!(!body["error"].as_str().unwrap().contains("ASCII"), "{body}");
    }
}

#[tokio::test]
async fn a_wrong_code_creates_no_guest_account() {
    let f = Fixture::new().await;
    let (host, _) = party(&f, &[]).await;
    let code = host["lobby"]["join_code"].as_str().unwrap().to_owned();
    let before = users(&f).await;
    let wrong = if code == "ZZZZZZ" { "YYYYYY" } else { "ZZZZZZ" };
    let (status, body) = guest_sign_in(&f, &format!("?join_code={wrong}")).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (404, Some("lobby_unavailable"))
    );
    let (status, body) = guest_sign_in(&f, "?join_code=AB").await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("invalid_join_code"))
    );
    assert_eq!(users(&f).await, before);
    // The right code, typed loosely, signs in.
    let loose = format!("{}-{}", &code[..3], code[3..].to_lowercase());
    let (status, body) = guest_sign_in(&f, &format!("?join_code={loose}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(users(&f).await, before + 1);
    // A closed lobby's code is refused too.
    let (status, _) = delete(
        &f,
        f.user,
        &format!(
            "/api/lobbies/memberships/{}",
            host["membership_id"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = guest_sign_in(&f, &format!("?join_code={code}")).await;
    assert_eq!(status, 404);
    assert_eq!(users(&f).await, before + 1);
    // Without a code it still signs in, as before.
    assert_eq!(guest_sign_in(&f, "").await.0, 200);
}

#[tokio::test]
async fn guest_status_says_whether_a_session_cookie_exists() {
    let f = Fixture::new().await;
    let status = |cookie: Option<&'static str>| {
        let mut request = f.client.get(format!("{}/api/auth/guest", f.base));
        if let Some(cookie) = cookie {
            request = request.header("Cookie", cookie);
        }
        async move { request.send().await.unwrap().json::<Value>().await.unwrap() }
    };
    assert_eq!(status(None).await["session"], false);
    assert_eq!(
        status(Some("crescend_refresh=anything")).await["session"],
        true
    );
}

#[tokio::test]
async fn the_host_removes_a_member_like_a_leave_and_their_stream_says_removed() {
    let f = Fixture::new().await;
    let (host, guests) = party(&f, &["Kat", "Ghost"]).await;
    let hm = host["membership_id"].as_str().unwrap().to_owned();
    let code = host["lobby"]["join_code"].as_str().unwrap().to_owned();
    let (kat, km) = guests[0].clone();
    let (ghost, gm) = guests[1].clone();
    let mut stream = connect(&f, &gm, ghost).await;
    assert_eq!(stream.status(), 200);
    assert!(frame(&mut stream).await.contains("sync_required"));
    let revision: u64 = get(&f, f.user, "/api/lobbies/current").await["lobby"]["revision"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Only the host can remove, and never themself.
    let (status, body) = delete(
        &f,
        kat,
        &format!("/api/lobbies/memberships/{km}/members/{ghost}"),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("host_required"))
    );
    let (status, body) = delete(
        &f,
        f.user,
        &format!("/api/lobbies/memberships/{hm}/members/{}", f.user),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("cannot_remove_host"))
    );
    let (status, body) = delete(
        &f,
        f.user,
        &format!("/api/lobbies/memberships/{km}/members/{ghost}"),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("membership_forbidden"))
    );
    for bad in ["abc", "-1", "0", "+5"] {
        let (status, _) = delete(
            &f,
            f.user,
            &format!("/api/lobbies/memberships/{hm}/members/{bad}"),
        )
        .await;
        assert_eq!(status, 400, "{bad}");
    }

    let (status, _) = delete(
        &f,
        f.user,
        &format!("/api/lobbies/memberships/{hm}/members/{ghost}"),
    )
    .await;
    assert_eq!(status, 204);
    let view = get(&f, f.user, "/api/lobbies/current").await;
    assert_eq!(names(&view), ["Daniel", "Kat"]);
    let after: u64 = view["lobby"]["revision"].as_str().unwrap().parse().unwrap();
    assert_eq!(after, revision + 1);
    let ended = frame(&mut stream).await;
    assert!(
        ended.contains("membership_ended") && ended.contains(r#""reason":"removed""#),
        "{ended}"
    );
    // The removed member is simply out, like after leaving.
    assert_eq!(
        get(&f, ghost, "/api/lobbies/current").await["lobby"],
        Value::Null
    );
    // Removing again is a no-op.
    let (status, _) = delete(
        &f,
        f.user,
        &format!("/api/lobbies/memberships/{hm}/members/{ghost}"),
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(
        get(&f, f.user, "/api/lobbies/current").await["lobby"]["revision"],
        view["lobby"]["revision"]
    );
    // They can come back with the code and get their name back; an ordinary leave
    // later still reads "left".
    let (status, back) = post(&f, ghost, "/api/lobbies/join", json!({"join_code": code})).await;
    assert_eq!(status, 200, "{back}");
    assert_eq!(names(&back), ["Daniel", "Kat", "Ghost"]);
    let nm = back["membership_id"].as_str().unwrap().to_owned();
    let mut stream = connect(&f, &nm, ghost).await;
    assert!(frame(&mut stream).await.contains("sync_required"));
    assert_eq!(
        delete(&f, ghost, &format!("/api/lobbies/memberships/{nm}"))
            .await
            .0,
        204
    );
    let ended = frame(&mut stream).await;
    assert!(ended.contains(r#""reason":"left""#), "{ended}");
}

#[tokio::test]
async fn removing_a_member_drops_their_skip_votes() {
    let f = Fixture::new().await;
    let (host, guests) = party(&f, &["A", "B", "C"]).await;
    let hm = host["membership_id"].as_str().unwrap().to_owned();
    let queue = get(&f, f.user, &format!("/api/lobbies/memberships/{hm}/queue")).await;
    assert_eq!(queue["voting"]["active_guest_count"], 3);
    let (_, _) = delete(
        &f,
        f.user,
        &format!("/api/lobbies/memberships/{hm}/members/{}", guests[2].0),
    )
    .await;
    let queue = get(&f, f.user, &format!("/api/lobbies/memberships/{hm}/queue")).await;
    assert_eq!(queue["voting"]["active_guest_count"], 2);
}

#[tokio::test]
async fn members_away_for_a_minute_stop_counting_toward_the_skip_majority() {
    let f = Fixture::new().await;
    let (host, guests) = party(&f, &["A", "B", "C"]).await;
    let hm = host["membership_id"].as_str().unwrap().to_owned();
    let lobby = host["lobby"]["id"].as_str().unwrap().to_owned();
    let count = || async {
        get(&f, f.user, &format!("/api/lobbies/memberships/{hm}/queue")).await["voting"]
            ["active_guest_count"]
            .as_i64()
            .unwrap()
    };
    // Away for less than the grace period: still counted.
    crate::lobbies::presence::mark_away(&lobby, guests[0].0, 30);
    assert_eq!(count().await, 3);
    crate::lobbies::presence::mark_away(&lobby, guests[0].0, 61);
    assert_eq!(count().await, 2);
    // The settle the stream's drop schedules bumps the revision so everyone refetches.
    let before = get(&f, f.user, "/api/lobbies/current").await["lobby"]["revision"].clone();
    let revision = f
        .database
        .settle_presence(&lobby, || f.time.load(Ordering::SeqCst))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        revision.to_string(),
        (before.as_str().unwrap().parse::<i64>().unwrap() + 1).to_string()
    );
    // Coming back (a new stream) counts again at once.
    let mut stream = connect(&f, &guests[0].1, guests[0].0).await;
    assert!(frame(&mut stream).await.contains("sync_required"));
    assert_eq!(count().await, 3);
    // Leaving forgets them.
    drop(stream);
    crate::lobbies::presence::mark_away(&lobby, guests[1].0, 120);
    assert_eq!(
        delete(
            &f,
            guests[1].0,
            &format!("/api/lobbies/memberships/{}", guests[1].1)
        )
        .await
        .0,
        204
    );
    assert!(!crate::lobbies::presence::away(&lobby).contains(&guests[1].0));
}

#[tokio::test]
async fn emoji_nicknames_and_their_suffixes() {
    let f = Fixture::new().await;
    let family = "👨\u{200D}👩\u{200D}👧";
    let rainbow = "🏳\u{FE0F}\u{200D}🌈 Kat";
    let (host, _) = party(&f, &[family, rainbow, "Kat ❤\u{FE0F}"]).await;
    let code = host["lobby"]["join_code"].as_str().unwrap().to_owned();
    let view = get(&f, f.user, "/api/lobbies/current").await;
    assert_eq!(names(&view), ["Daniel", family, rainbow, "Kat ❤\u{FE0F}"]);
    // A clash gets " 2" without splitting the emoji.
    let long = "👨\u{200D}👩\u{200D}👧\u{200D}👦".repeat(24);
    let a = f.database.create_user("a").await.unwrap().id;
    let b = f.database.create_user("b").await.unwrap().id;
    let (status, _) = post(
        &f,
        a,
        "/api/lobbies/join",
        json!({"join_code": code, "nickname": long}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, view) = post(
        &f,
        b,
        "/api/lobbies/join",
        json!({"join_code": code, "nickname": long}),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    let last = names(&view).pop().unwrap();
    assert_eq!(crate::database::nicknames::length(&last), 24);
    assert!(last.ends_with(" 2"));
    // Invisible tricks are still refused.
    for bad in ["\u{200B}", "Kat\u{200D}", "Kat\u{FE0F}", "a\u{202E}b"] {
        let (status, body) = post(
            &f,
            b,
            "/api/lobbies/join",
            json!({"join_code": code, "nickname": bad}),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("invalid_nickname")),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn nickname_tables_from_before_emoji_are_widened_in_place() {
    let f = Fixture::new().await;
    let pool = &f.database.db_pool;
    for statement in [
        "DROP TABLE lobby_nicknames",
        "DROP TABLE user_nicknames",
        "CREATE TABLE lobby_nicknames (
            lobby_id TEXT NOT NULL REFERENCES lobbies(id) ON DELETE CASCADE,
            user_id INTEGER NOT NULL REFERENCES users(id),
            nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 24),
            folded TEXT NOT NULL,
            PRIMARY KEY(lobby_id, user_id),
            UNIQUE(lobby_id, folded)
        ) STRICT",
        "CREATE TABLE user_nicknames (
            user_id INTEGER PRIMARY KEY NOT NULL REFERENCES users(id),
            nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 24),
            updated_at INTEGER NOT NULL
        ) STRICT",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO user_nicknames VALUES (?, 'Old Kat', 1)")
        .bind(f.user)
        .execute(pool)
        .await
        .unwrap();
    f.database.create_tables().await.unwrap();
    f.database.create_tables().await.unwrap();
    let sql: Vec<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE name IN ('lobby_nicknames', 'user_nicknames')",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(sql.len(), 2);
    assert!(
        sql.iter().all(|s| s.contains("BETWEEN 1 AND 288)")),
        "{sql:?}"
    );
    assert_eq!(
        f.database.last_nickname(f.user).await.unwrap().as_deref(),
        Some("Old Kat")
    );
    // And a long emoji name now fits.
    let (status, view) = post(
        &f,
        f.user,
        "/api/lobbies",
        json!({"nickname": "👨\u{200D}👩\u{200D}👧\u{200D}👦".repeat(10)}),
    )
    .await;
    assert_eq!(status, 201, "{view}");
}
