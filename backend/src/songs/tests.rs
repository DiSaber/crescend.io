//! Song catalog acceptance tests (#41 part 1): starter list, recording accepted
//! submissions, and authenticated FTS5 search.
use super::*;
use crate::{
    models::youtube::VideoMetadata,
    queue::tests::{Fixture, NOW},
    youtube::YouTubeVideo,
};
use reqwest::Method;
use serde_json::{Value, json};

fn video(id: &str, title: &str, artist: &str) -> VideoMetadata {
    VideoMetadata {
        video_id: id.into(),
        title: title.into(),
        author_name: artist.into(),
        author_url: "https://youtube.com/@channel".into(),
        thumbnail_url: format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
        embed_url: format!("https://youtube.com/embed/{id}"),
    }
}

async fn row(f: &Fixture, id: &str) -> Option<(String, String, i64, Option<String>)> {
    sqlx::query_as(
        "SELECT title, artist, times_queued, last_queued_at FROM songs WHERE video_id = ?",
    )
    .bind(id)
    .fetch_optional(&f.database.db_pool)
    .await
    .unwrap()
}

async fn search(
    f: &Fixture,
    base: &str,
    user: &crate::auth::AuthenticatedUser,
    params: &[(&str, &str)],
) -> (u16, Value, reqwest::header::HeaderMap) {
    let url = url::Url::parse_with_params(&format!("{base}/api/songs/search"), params).unwrap();
    let response = f
        .request(Method::GET, url.to_string(), user)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    (
        status,
        response.json().await.unwrap_or(Value::Null),
        headers,
    )
}

fn titles(body: &Value) -> Vec<String> {
    body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn starter_list_is_well_formed() {
    let songs = starter_songs();
    assert!(
        (50..=100).contains(&songs.len()),
        "{} starter songs",
        songs.len()
    );
    let mut ids = std::collections::HashSet::new();
    for song in &songs {
        assert!(ids.insert(&song.video_id), "duplicate {}", song.video_id);
        let parsed = YouTubeVideo::parse(&watch_url(&song.video_id)).unwrap();
        assert_eq!(parsed.id, song.video_id);
        for text in [&song.title, &song.artist] {
            assert!(!text.is_empty() && text.trim() == text, "{text:?}");
            assert_eq!(
                &decode_html_entities(text),
                text,
                "undecoded entity in {text:?}"
            );
        }
    }
}

#[test]
fn html_entities_decode_once() {
    assert_eq!(
        decode_html_entities("Tom &amp; Jerry&#39;s &quot;Song&quot; &#x1F600; &lt;3"),
        "Tom & Jerry's \"Song\" \u{1F600} <3"
    );
    assert_eq!(decode_html_entities("&amp;amp;"), "&amp;");
    for kept in [
        "R&B",
        "a & b",
        "&bogus;",
        "&#0;",
        "&#xD800;",
        "&#;",
        "&",
        "&verylongentityname;",
    ] {
        assert_eq!(decode_html_entities(kept), kept);
    }
}

#[test]
fn search_text_never_reaches_fts_syntax() {
    let search = SongSearch::parse(" \"crab\" OR NEAR(rave* ", None).unwrap();
    assert_eq!(
        search.prefix_query(),
        "\"crab\"* \"OR\"* \"NEAR\"* \"rave\"*"
    );
    assert_eq!(search.word_query(), "\"crab\" \"OR\" \"NEAR\" \"rave\"");
    assert_eq!(search.limit, DEFAULT_LIMIT);
    assert!(SongSearch::parse("!!!", Some(5)).unwrap().is_empty());
    for (q, limit) in [("", None), ("   ", None), ("x", Some(0)), ("x", Some(26))] {
        assert!(matches!(
            SongSearch::parse(q, limit),
            Err(SongsError::InvalidQuery)
        ));
    }
    assert!(SongSearch::parse(&"é".repeat(100), None).is_ok());
    assert!(SongSearch::parse(&"é".repeat(101), None).is_err());
}

#[tokio::test]
async fn starter_load_is_idempotent_and_preserves_counts() {
    let f = Fixture::new().await;
    let songs = starter_songs();
    assert_eq!(
        f.database.load_starter_songs(&songs).await.unwrap(),
        songs.len() as u64
    );
    assert_eq!(f.database.load_starter_songs(&songs).await.unwrap(), 0);
    f.database.create_tables().await.unwrap();
    let crab = "LDU_Txk06tM";
    assert_eq!(row(&f, crab).await.unwrap().2, 0);
    f.database
        .record_queued_song(
            &video(
                crab,
                "Noisestorm - Crab Rave [Monstercat Release]",
                "Monstercat Instinct",
            ),
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(f.database.load_starter_songs(&songs).await.unwrap(), 0);
    let (title, artist, count, at) = row(&f, crab).await.unwrap();
    assert_eq!(
        (title.as_str(), artist.as_str(), count),
        ("Crab Rave", "Noisestorm", 1)
    );
    assert_eq!(at.as_deref(), Some("2027-01-15T08:00:00+00:00"));
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM songs")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(total, songs.len() as i64);
}

#[tokio::test]
async fn accepted_submissions_are_recorded_once_each() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.upstream.reset().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
            "title": "Tom &amp; Jerry&#39;s Theme", "author_name": "Cartoon &quot;Classics&quot;",
            "author_url": "https://youtube.com/@classics", "thumbnail_url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        })))
        .mount(&f.upstream)
        .await;
    let http = f.server().await;
    let url = format!("{}/entries", http.queue(&membership));
    for request_id in [
        "00000000000000000000000000000001",
        "00000000000000000000000000000002",
        "00000000000000000000000000000002",
    ] {
        let response = f
            .request(Method::POST, url.clone(), &host)
            .json(&json!({"url": "https://youtu.be/dQw4w9WgXcQ", "request_id": request_id}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    // Two accepted occurrences; the matching retry is not another acceptance.
    let (title, artist, count, at) = row(&f, "dQw4w9WgXcQ").await.unwrap();
    assert_eq!(title, "Tom & Jerry's Theme");
    assert_eq!(artist, "Cartoon \"Classics\"");
    assert_eq!(count, 2);
    assert_eq!(at.as_deref(), Some("2027-01-15T08:00:00+00:00"));
}

#[tokio::test]
async fn catalog_failure_never_fails_the_submission() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    f.mock_metadata().await;
    sqlx::query("CREATE TRIGGER fail_songs BEFORE INSERT ON songs BEGIN SELECT RAISE(ABORT, 'injected catalog failure'); END")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let http = f.server().await;
    let response = f
        .request(Method::POST, format!("{}/entries", http.queue(&membership)), &host)
        .json(&json!({"url": "https://youtu.be/dQw4w9WgXcQ", "request_id": "00000000000000000000000000000001"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["changed"], true);
    assert_eq!(body["queue"]["current"]["video"]["video_id"], "dQw4w9WgXcQ");
    assert!(row(&f, "dQw4w9WgXcQ").await.is_none());
}

#[tokio::test]
async fn search_matches_partial_and_accentless_words_in_title_or_artist() {
    let f = Fixture::new().await;
    f.database
        .load_starter_songs(&starter_songs())
        .await
        .unwrap();
    let (guest, _) = f.guest("guest", &f.host().await.2).await;
    let http = f.server().await;
    for (q, expected) in [
        ("cra ra", "Crab Rave"),
        ("CRAB RAVE", "Crab Rave"),
        ("beyonce", "Crazy in Love"),
        ("senorita", "Señorita"),
        ("dakiti", "Dákiti"),
        ("rose apt", "APT."),
        ("don't stop me", "Don't Stop Me Now"),
    ] {
        let (status, body, _) = search(&f, &http.base, &guest, &[("q", q)]).await;
        assert_eq!(status, 200, "{q}");
        assert_eq!(
            titles(&body).first().map(String::as_str),
            Some(expected),
            "{q}: {body}"
        );
    }
    let (_, body, headers) = search(&f, &http.base, &guest, &[("q", "shakira")]).await;
    let found = titles(&body);
    assert_eq!(found.len(), 3, "{found:?}");
    for title in [
        "Dai Dai",
        "Hips Don't Lie",
        "Waka Waka (This Time for Africa)",
    ] {
        assert!(found.iter().any(|t| t == title), "{title} in {found:?}");
    }
    assert_eq!(body["youtube"], "not_configured");
    assert_eq!(headers["cache-control"], "no-store");
    let first = &body["results"][0];
    let parsed = YouTubeVideo::parse(first["url"].as_str().unwrap()).unwrap();
    assert_eq!(parsed.id, first["video_id"].as_str().unwrap());
    assert!(
        first["thumbnail_url"]
            .as_str()
            .unwrap()
            .starts_with("https://i.ytimg.com/vi/")
    );
    assert_eq!(first["times_queued"], 0);
}

#[tokio::test]
async fn whole_words_rank_first_then_most_queued() {
    let f = Fixture::new().await;
    let user = f.account("member").await;
    for (id, title, plays) in [
        ("aaaaaaaaaaa", "Love Story", 0),
        ("bbbbbbbbbbb", "Lovely Day", 5),
        ("ccccccccccc", "Love Me Do", 3),
        ("ddddddddddd", "Tainted Love", 3),
    ] {
        f.database
            .load_starter_songs(&[StarterSong {
                video_id: id.into(),
                title: title.into(),
                artist: "Artist".into(),
            }])
            .await
            .unwrap();
        for _ in 0..plays {
            f.database
                .record_queued_song(&video(id, title, "Artist"), NOW)
                .await
                .unwrap();
        }
    }
    let http = f.server().await;
    let (_, body, _) = search(&f, &http.base, &user, &[("q", "love")]).await;
    let found = titles(&body);
    // Whole-word "love" first (most queued, then relevance), prefix-only "Lovely" last.
    assert_eq!(
        found.last().map(String::as_str),
        Some("Lovely Day"),
        "{found:?}"
    );
    assert_eq!(found[2], "Love Story", "{found:?}");
    assert_eq!(found.len(), 4);
    let (_, body, _) = search(&f, &http.base, &user, &[("q", "love"), ("limit", "2")]).await;
    assert_eq!(titles(&body).len(), 2);
    let (_, body, _) = search(&f, &http.base, &user, &[("q", "lov")]).await;
    assert_eq!(
        titles(&body)[0],
        "Lovely Day",
        "all prefix-only: most queued first"
    );
}

#[tokio::test]
async fn search_rejects_anonymous_unknown_and_malformed_requests() {
    let f = Fixture::new().await;
    f.database
        .load_starter_songs(&starter_songs())
        .await
        .unwrap();
    let user = f.account("member").await;
    let http = f.server().await;
    let anonymous = reqwest::Client::new()
        .get(format!("{}/api/songs/search?q=crab", http.base))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    let unknown = crate::auth::AuthenticatedUser {
        id: i64::MAX,
        expires_at: NOW + 3600,
    };
    let (status, body, headers) = search(&f, &http.base, &unknown, &[("q", "crab")]).await;
    assert_eq!((status, body["code"].as_str()), (401, Some("unauthorized")));
    assert!(headers.contains_key("www-authenticate"));
    let long = "a".repeat(101);
    for params in [
        vec![],
        vec![("q", "  ")],
        vec![("q", long.as_str())],
        vec![("q", "crab"), ("limit", "0")],
        vec![("q", "crab"), ("limit", "26")],
        vec![("q", "crab"), ("limit", "ten")],
    ] {
        let (status, body, _) = search(&f, &http.base, &user, &params).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("invalid_search")),
            "{params:?}"
        );
    }
    for q in ["!!!", "\"crab\" OR NEAR(", "zzzzzzzzqqq"] {
        let (status, body, _) = search(&f, &http.base, &user, &[("q", q)]).await;
        assert_eq!(status, 200, "{q}");
        assert!(titles(&body).len() <= 1, "{q}: {body}");
    }
}
