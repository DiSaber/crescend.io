//! Supported-link and bounded mock-upstream acceptance tests.
use super::*;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

#[test]
fn supported_links_normalize_and_reject_foreign_links() {
    for url in [
        "https://youtu.be/dQw4w9WgXcQ?t=4",
        "https://youtube.com/watch?v=dQw4w9WgXcQ",
        "https://m.youtube.com/shorts/dQw4w9WgXcQ",
        "https://www.youtube.com/embed/dQw4w9WgXcQ",
    ] {
        let video = YouTubeVideo::parse(url).expect("supported URL must parse");
        assert_eq!(video.id, "dQw4w9WgXcQ");
        assert_eq!(
            video.watch_url.as_str(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }
    for url in [
        "https://example.com/watch?v=dQw4w9WgXcQ",
        "https://youtube.com/playlist?list=abc",
        "https://youtu.be/short",
        "javascript:alert(1)",
    ] {
        assert!(YouTubeVideo::parse(url).is_err());
    }
}

fn video() -> YouTubeVideo {
    YouTubeVideo {
        id: "dQw4w9WgXcQ".into(),
        watch_url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
            .parse()
            .unwrap(),
        embed_url: "https://www.youtube.com/embed/dQw4w9WgXcQ?autoplay=1&enablejsapi=1".into(),
    }
}

#[tokio::test]
async fn resolves_authoritative_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "title":"Authoritative title", "author_name":"Channel", "author_url":"https://youtube.com/@channel", "thumbnail_url":"https://i.ytimg.com/image.jpg"
    }))).mount(&server).await;
    let client = MetadataClient::with_endpoint(server.uri().parse().unwrap());
    let metadata = client
        .lookup(&video())
        .await
        .expect("valid metadata must resolve");
    assert_eq!(metadata.title, "Authoritative title");
    assert_eq!(metadata.video_id, "dQw4w9WgXcQ");
    assert!(metadata.embed_url.contains("enablejsapi=1"));
}

#[tokio::test]
async fn bounds_and_validates_upstream_response() {
    for response in [
        ResponseTemplate::new(404),
        ResponseTemplate::new(200).set_body_string("{"),
        ResponseTemplate::new(200).set_body_string("x".repeat(65537)),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(response)
            .mount(&server)
            .await;
        assert!(matches!(
            MetadataClient::with_endpoint(server.uri().parse().unwrap())
                .lookup(&video())
                .await,
            Err(MetadataError::Unavailable)
        ));
    }
}

#[tokio::test]
async fn total_timeout_is_bounded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(7)))
        .mount(&server)
        .await;
    let start = std::time::Instant::now();
    assert!(matches!(
        MetadataClient::with_endpoint(server.uri().parse().unwrap())
            .lookup(&video())
            .await,
        Err(MetadataError::Service)
    ));
    assert!(start.elapsed() < std::time::Duration::from_secs(6));
}
