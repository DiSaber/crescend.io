//! Guest sign-in, nicknames and rejoin identity (docs/guest-join.md).
use super::*;
use serde_json::{Value, json};

/// A browser: its cookies, kept by hand because the test client has no cookie store.
#[derive(Default, Clone)]
struct Browser {
    refresh: Option<String>,
    guest: Option<String>,
}

struct SignIn {
    status: u16,
    body: Value,
    retry_after: Option<String>,
    user: Option<i64>,
}

fn cookie_value(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.strip_prefix(&format!("{name}=")))
        .map(|rest| rest.split(';').next().unwrap().to_owned())
}

impl Fixture {
    async fn guest_sign_in(&self, browser: &mut Browser, ip: &str) -> SignIn {
        let mut cookies = Vec::new();
        if let Some(v) = &browser.refresh {
            cookies.push(format!("crescend_refresh={v}"));
        }
        if let Some(v) = &browser.guest {
            cookies.push(format!("crescend_guest={v}"));
        }
        let mut request = self
            .client
            .post(format!("{}/api/auth/guest", self.base))
            .header("X-CSRF-Protection", "1")
            .header("X-Crescend-Client-IP", ip);
        if !cookies.is_empty() {
            request = request.header("Cookie", cookies.join("; "));
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .map(|v| v.to_str().unwrap().to_owned());
        if let Some(v) = cookie_value(&response, "crescend_refresh") {
            browser.refresh = Some(v);
        }
        if let Some(v) = cookie_value(&response, "crescend_guest") {
            browser.guest = Some(v);
        }
        let body: Value = response.json().await.unwrap();
        let user = body["access_token"]
            .as_str()
            .map(|t| self.auth.jwt.verify(t).unwrap().id);
        SignIn {
            status,
            body,
            retry_after,
            user,
        }
    }

    async fn post(&self, user: i64, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .client
            .post(format!("{}{path}", self.base))
            .bearer_auth(self.auth.jwt.issue(user).unwrap())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, user: i64, path: &str) -> Value {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .bearer_auth(self.auth.jwt.issue(user).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        response.json().await.unwrap()
    }

    async fn users(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&self.database.db_pool)
            .await
            .unwrap()
    }

    async fn named_lobby(&self, nickname: &str) -> Value {
        let (status, body) = self
            .post(self.user, "/api/lobbies", json!({"nickname": nickname}))
            .await;
        assert_eq!(status, 201, "{body}");
        body
    }
}

fn nicknames(view: &Value) -> Vec<String> {
    view["lobby"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["nickname"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn guest_sign_in_creates_once_and_the_same_browser_gets_the_same_account() {
    let f = Fixture::new().await;
    let before = f.users().await;
    let mut browser = Browser::default();
    let first = f.guest_sign_in(&mut browser, "198.51.100.1").await;
    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(first.body["token_type"], "Bearer");
    assert_eq!(first.body.as_object().unwrap().len(), 3, "access only");
    let guest = first.user.unwrap();
    assert!(browser.refresh.is_some() && browser.guest.is_some());
    assert_eq!(f.users().await, before + 1);
    let subject: String = sqlx::query_scalar("SELECT google_sub FROM users WHERE id = ?")
        .bind(guest)
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert!(subject.starts_with("guest:"));
    assert!(
        !subject.contains(browser.guest.as_deref().unwrap()),
        "only the hash is stored"
    );

    let me = f.get(guest, "/api/me").await;
    assert_eq!(
        me,
        json!({"user_id": guest.to_string(), "guest": true, "last_nickname": null})
    );
    assert_eq!(f.get(f.user, "/api/me").await["guest"], false);

    // The refresh cookie works like a Google one.
    let refreshed = f
        .client
        .post(format!("{}/api/auth/refresh", f.base))
        .header("X-CSRF-Protection", "1")
        .header(
            "Cookie",
            format!("crescend_refresh={}", browser.refresh.as_ref().unwrap()),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(refreshed.status(), 200);
    let token: Value = refreshed.json().await.unwrap();
    assert_eq!(
        f.auth
            .jwt
            .verify(token["access_token"].as_str().unwrap())
            .unwrap()
            .id,
        guest
    );

    // After the session is gone (logout, expiry), the guest cookie restores the account.
    let mut returning = Browser {
        refresh: None,
        guest: browser.guest.clone(),
    };
    let again = f.guest_sign_in(&mut returning, "198.51.100.1").await;
    assert_eq!((again.status, again.user), (200, Some(guest)));
    assert_eq!(f.users().await, before + 1);

    // Another browser is another guest.
    let other = f
        .guest_sign_in(&mut Browser::default(), "198.51.100.1")
        .await;
    assert_ne!(other.user.unwrap(), guest);

    // Same CSRF and empty-body rules as refresh.
    let response = f
        .client
        .post(format!("{}/api/auth/guest", f.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = f
        .client
        .post(format!("{}/api/auth/guest", f.base))
        .header("X-CSRF-Protection", "1")
        .body("{\"nickname\":\"x\"}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let response = f
        .client
        .post(format!("{}/api/auth/guest", f.base))
        .header("X-CSRF-Protection", "1")
        .header("Sec-Fetch-Site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn guest_status_reports_the_last_nickname_without_signing_in() {
    let f = Fixture::new().await;
    let status = |cookie: Option<String>| {
        let mut request = f.client.get(format!("{}/api/auth/guest", f.base));
        if let Some(c) = cookie {
            request = request.header("Cookie", format!("crescend_guest={c}"));
        }
        async move {
            let response = request.send().await.unwrap();
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert!(response.headers().get("set-cookie").is_none());
            response.json::<Value>().await.unwrap()
        }
    };
    assert_eq!(
        status(None).await,
        json!({"returning": false, "last_nickname": null, "session": false})
    );
    let mut browser = Browser::default();
    let guest = f
        .guest_sign_in(&mut browser, "198.51.100.2")
        .await
        .user
        .unwrap();
    assert_eq!(
        status(browser.guest.clone()).await,
        json!({"returning": true, "last_nickname": null, "session": false})
    );
    let lobby = f.named_lobby("DJ").await;
    let code = lobby["lobby"]["join_code"].as_str().unwrap();
    f.post(
        guest,
        "/api/lobbies/join",
        json!({"join_code": code, "nickname": "  Kat  "}),
    )
    .await;
    assert_eq!(
        status(browser.guest.clone()).await,
        json!({"returning": true, "last_nickname": "Kat", "session": false})
    );
    assert_eq!(f.get(guest, "/api/me").await["last_nickname"], "Kat");
    assert_eq!(status(Some("f".repeat(64))).await["returning"], false);
}

#[tokio::test]
async fn new_guest_accounts_are_limited_per_address() {
    let f = Fixture::new().await;
    let mut first = Browser::default();
    assert_eq!(f.guest_sign_in(&mut first, "203.0.113.9").await.status, 200);
    for _ in 1..crate::auth::guest::NEW_GUESTS_PER_WINDOW {
        assert_eq!(
            f.guest_sign_in(&mut Browser::default(), "203.0.113.9")
                .await
                .status,
            200
        );
    }
    let users = f.users().await;
    let limited = f
        .guest_sign_in(&mut Browser::default(), "203.0.113.9")
        .await;
    assert_eq!(limited.status, 429);
    assert_eq!(limited.body["code"], "too_many_guests");
    assert_eq!(limited.retry_after.as_deref(), Some("3600"));
    assert_eq!(f.users().await, users, "nothing created");
    // Returning guests are not new accounts; other addresses have their own budget.
    assert_eq!(
        f.guest_sign_in(&mut first.clone(), "203.0.113.9")
            .await
            .status,
        200
    );
    assert_eq!(
        f.guest_sign_in(&mut Browser::default(), "203.0.113.10")
            .await
            .status,
        200
    );
    // The window slides.
    f.time.fetch_add(3600, Ordering::SeqCst);
    assert_eq!(
        f.guest_sign_in(&mut Browser::default(), "203.0.113.9")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn guests_join_but_only_google_accounts_host() {
    let f = Fixture::new().await;
    let guest = f
        .guest_sign_in(&mut Browser::default(), "198.51.100.3")
        .await
        .user
        .unwrap();
    for body in [json!(null), json!({"nickname": "Sneaky"})] {
        let mut request = f
            .client
            .post(format!("{}/api/lobbies", f.base))
            .bearer_auth(f.auth.jwt.issue(guest).unwrap());
        if !body.is_null() {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "google_account_required"
        );
    }
    assert_eq!(f.count("lobbies").await, 0);
    let lobby = f.named_lobby("Daniel").await;
    let (status, view) = f
        .post(
            guest,
            "/api/lobbies/join",
            json!({"join_code": lobby["lobby"]["join_code"], "nickname": "Sam"}),
        )
        .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(nicknames(&view), ["Daniel", "Sam"]);
}

#[tokio::test]
async fn nickname_rules_defaults_and_uniqueness() {
    let f = Fixture::new().await;
    // An empty create body still works: "Host" until the account picks a name.
    let response = f.create().await;
    assert_eq!(response.status(), 201);
    let view: Value = response.json().await.unwrap();
    assert_eq!(nicknames(&view), ["Host"]);
    let code = view["lobby"]["join_code"].as_str().unwrap().to_owned();

    let mut guests = Vec::new();
    for i in 0..6 {
        guests.push(
            f.database
                .create_user(&format!("nick-{i}"))
                .await
                .unwrap()
                .id,
        );
    }
    let join = |user: i64, nickname: Value| {
        let mut body = json!({"join_code": code});
        if !nickname.is_null() {
            body["nickname"] = nickname;
        }
        f.post(user, "/api/lobbies/join", body)
    };
    for bad in [
        json!(""),
        json!("   "),
        json!("a\u{0007}b"),
        json!("x".repeat(25)),
        json!("rtl\u{202E}"),
        json!(42),
    ] {
        let (status, body) = join(guests[0], bad.clone()).await;
        assert_eq!(status, 400, "{bad}");
        let expected = if bad.is_string() {
            "invalid_nickname"
        } else {
            "invalid_request"
        };
        assert_eq!(body["code"], expected, "{bad}");
    }
    assert_eq!(
        f.count("lobby_memberships").await,
        1,
        "rejected joins add nobody"
    );

    join(guests[0], json!("  Sam   Lee ")).await;
    join(guests[1], json!("sam lee")).await;
    join(guests[2], json!("SAM LEE")).await;
    join(guests[3], json!("host")).await;
    let long = "abcdefghijklmnopqrstuvwx";
    join(guests[4], json!(long)).await;
    let (_, view) = join(guests[5], json!(long)).await;
    assert_eq!(
        nicknames(&view),
        [
            "Host",
            "Sam Lee",
            "sam lee 2",
            "SAM LEE 3",
            "host 2",
            long,
            "abcdefghijklmnopqrstuv 2"
        ]
    );

    // Renaming in the same lobby is a change; the old name is freed, a taken one suffixed.
    let revision = view["lobby"]["revision"].as_str().unwrap().to_owned();
    let (_, renamed) = join(guests[0], json!("Sammy")).await;
    assert_ne!(renamed["lobby"]["revision"], revision);
    assert_eq!(nicknames(&renamed)[1], "Sammy");
    let (_, unchanged) = join(guests[0], Value::Null).await;
    assert_eq!(unchanged["lobby"]["revision"], renamed["lobby"]["revision"]);
    assert_eq!(nicknames(&unchanged)[1], "Sammy");
    let (_, taken) = join(guests[1], json!("SAMMY")).await;
    assert_eq!(nicknames(&taken)[2], "SAMMY 2");

    // Names are per lobby: the same account uses another name elsewhere and the host's
    // next lobby offers their last choice.
    let other_host = f.database.create_user("second-host").await.unwrap().id;
    let (_, other) = f
        .post(other_host, "/api/lobbies", json!({"nickname": "Sam Lee"}))
        .await;
    let other_code = other["lobby"]["join_code"].as_str().unwrap();
    let (_, moved) = f
        .post(
            guests[0],
            "/api/lobbies/join",
            json!({"join_code": other_code, "nickname": "Elsewhere"}),
        )
        .await;
    let mut names = nicknames(&moved);
    names.sort();
    assert_eq!(names, ["Elsewhere", "Sam Lee"]);
    let current = f.get(f.user, "/api/lobbies/current").await;
    assert_eq!(nicknames(&current)[0], "Host");

    // Hostile markup is stored as the text it is.
    let (status, view) = join(guests[3], json!("<svg onload=alert(1)>")).await;
    assert_eq!(status, 200);
    assert!(nicknames(&view).contains(&"<svg onload=alert(1)>".to_owned()));
    let (status, body) = f
        .post(f.user, "/api/lobbies", json!({"nickname": "\u{0000}"}))
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("invalid_nickname"))
    );
}

#[tokio::test]
async fn rejoining_restores_votes_and_names_show_in_queue_responses() {
    let f = Fixture::new().await;
    let lobby = f.named_lobby("Daniel").await;
    let code = lobby["lobby"]["join_code"].as_str().unwrap().to_owned();
    let host_membership = lobby["membership_id"].as_str().unwrap().to_owned();
    let mut browser = Browser::default();
    let guest = f
        .guest_sign_in(&mut browser, "198.51.100.4")
        .await
        .user
        .unwrap();
    let (_, joined) = f
        .post(
            guest,
            "/api/lobbies/join",
            json!({"join_code": code, "nickname": "Kat"}),
        )
        .await;
    let membership = joined["membership_id"].as_str().unwrap().to_owned();
    // A second guest keeps the majority threshold above one vote, so the vote stays put.
    let bystander = f.database.create_user("bystander").await.unwrap().id;
    f.post(
        bystander,
        "/api/lobbies/join",
        json!({"join_code": code, "nickname": "Bo"}),
    )
    .await;

    let video = crate::models::youtube::VideoMetadata {
        video_id: "dQw4w9WgXcQ".into(),
        title: "Seed".into(),
        author_name: "Channel".into(),
        author_url: "https://youtube.com/@channel".into(),
        thumbnail_url: "https://i.ytimg.com/image.jpg".into(),
        embed_url: "https://youtube.com/embed/dQw4w9WgXcQ".into(),
    };
    let user = |id| crate::auth::AuthenticatedUser {
        id,
        expires_at: NOW + 3600,
    };
    for (who, member, request) in [(guest, &membership, 1), (f.user, &host_membership, 2)] {
        f.database
            .submit_queue(
                &user(who),
                member,
                &format!("{request:032x}"),
                &video,
                || NOW,
                || {
                    crate::models::queue::EntryId::random()
                        .map_err(|_| crate::queue::QueueError::Unavailable)
                },
            )
            .await
            .unwrap();
    }
    let queue_path = |m: &str| format!("/api/lobbies/memberships/{m}/queue");
    let queue = f.get(guest, &queue_path(&membership)).await;
    assert_eq!(queue["current"]["submitted_by_nickname"], "Kat");
    assert_eq!(queue["upcoming"][0]["submitted_by_nickname"], "Daniel");
    let entry = queue["upcoming"][0]["id"].as_str().unwrap().to_owned();
    let voted = f
        .client
        .post(format!(
            "{}{}/entries/{entry}/votes",
            f.base,
            queue_path(&membership)
        ))
        .bearer_auth(f.auth.jwt.issue(guest).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(voted.status(), 200);

    // The guest leaves, the session ends, and the same browser comes back.
    let left = f
        .client
        .delete(format!("{}/api/lobbies/memberships/{membership}", f.base))
        .bearer_auth(f.auth.jwt.issue(guest).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(left.status(), 204);
    let host_queue = f.get(f.user, &queue_path(&host_membership)).await;
    assert_eq!(
        host_queue["current"]["submitted_by_nickname"], "Kat",
        "added by keeps the name after leaving"
    );
    assert_eq!(host_queue["upcoming"][0]["active_vote_count"], 0);
    browser.refresh = None;
    let back = f
        .guest_sign_in(&mut browser, "198.51.100.4")
        .await
        .user
        .unwrap();
    assert_eq!(back, guest);
    let status: Value = f
        .client
        .get(format!("{}/api/auth/guest", f.base))
        .header(
            "Cookie",
            format!("crescend_guest={}", browser.guest.as_ref().unwrap()),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["last_nickname"], "Kat");
    let (_, rejoined) = f
        .post(
            back,
            "/api/lobbies/join",
            json!({"join_code": code, "nickname": "Kat"}),
        )
        .await;
    assert_eq!(
        nicknames(&rejoined),
        ["Daniel", "Kat", "Bo"],
        "its own name, not \"Kat 2\""
    );
    let membership = rejoined["membership_id"].as_str().unwrap();
    let queue = f.get(back, &queue_path(membership)).await;
    assert_eq!(queue["upcoming"][0]["has_voted"], true);
    assert_eq!(queue["upcoming"][0]["active_vote_count"], 1);
}
