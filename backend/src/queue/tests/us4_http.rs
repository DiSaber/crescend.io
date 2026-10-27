use super::*;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn advancement_requires_host_identity_and_explicit_reason() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&host, &membership, 3).await;
    let http = f.server().await;
    let body = json!({"expected_entry_id":ids[0].as_str(),"reason":"ended"});
    assert_eq!(
        f.request(
            Method::POST,
            format!("{}/advance", http.queue(&generation)),
            &guest
        )
        .json(&body)
        .send()
        .await
        .unwrap()
        .status(),
        403
    );
    let url = format!("{}/advance", http.queue(&membership));
    for body in [
        json!({"expected_entry_id":ids[0].as_str(),"reason":"error"}),
        json!({"expected_entry_id":ids[0].as_str(),"reason":"ended","extra":1}),
        json!({"reason":"ended"}),
    ] {
        assert_eq!(
            f.request(Method::POST, url.clone(), &host)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    for (id, reason) in ids.iter().zip(["ended", "skip", "playback_error_skip"]) {
        assert_eq!(
            f.request(Method::POST, url.clone(), &host)
                .json(&json!({"expected_entry_id":id.as_str(),"reason":reason}))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    assert_eq!(
        f.request(Method::POST, url, &host)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert!(
        f.database
            .current_lobby(host.id, || f.now())
            .await
            .unwrap()
            .lobby
            .is_some()
    );
}
