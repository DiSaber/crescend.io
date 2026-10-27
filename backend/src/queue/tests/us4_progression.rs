use super::*;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn veto_completion_races_and_delayed_veto_callbacks_cannot_skip_twice() {
    for _ in 0..5 {
        let f = Fixture::new().await;
        let (host, membership, _) = f.host().await;
        let ids = f.seed(&host, &membership, 3).await;
        let http = f.server().await;
        let base = http.queue(&membership);
        let (veto, advance) = tokio::join!(
            f.request(
                Method::DELETE,
                format!("{base}/entries/{}", ids[0].as_str()),
                &host
            )
            .send(),
            f.request(Method::POST, format!("{base}/advance"), &host)
                .json(&json!({"expected_entry_id":ids[0].as_str(),"reason":"ended"}))
                .send()
        );
        assert_eq!(veto.unwrap().status(), 200);
        assert!([200, 409].contains(&advance.unwrap().status().as_u16()));
        let queue = f
            .database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap();
        assert_eq!(queue.current.unwrap().id, ids[1]);
        assert_eq!(queue.revision.value(), 5);
        f.request(
            Method::DELETE,
            format!("{base}/entries/{}", ids[1].as_str()),
            &host,
        )
        .send()
        .await
        .unwrap();
        assert_eq!(
            f.request(Method::POST, format!("{base}/advance"), &host)
                .json(&json!({"expected_entry_id":ids[1].as_str(),"reason":"ended"}))
                .send()
                .await
                .unwrap()
                .status(),
            409
        );
        assert_eq!(
            f.database
                .queue_view(&host, &membership, || f.now())
                .await
                .unwrap()
                .current
                .unwrap()
                .id,
            ids[2]
        );
    }
}

#[tokio::test]
async fn concurrent_callbacks_consume_once_and_retries_cannot_skip_replacement() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    let ids = f.seed(&host, &membership, 3).await;
    let http = f.server().await;
    let url = format!("{}/advance", http.queue(&membership));
    let body = json!({"expected_entry_id":ids[0].as_str(),"reason":"ended"});
    let (a, b) = tokio::join!(
        f.request(Method::POST, url.clone(), &host)
            .json(&body)
            .send(),
        f.request(Method::POST, url.clone(), &host)
            .json(&body)
            .send()
    );
    let mut statuses = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    assert_eq!(
        f.request(Method::POST, url, &host)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let view = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(view.current.unwrap().id, ids[1]);
    assert_eq!(view.upcoming.len(), 1);
    let receipt: Value = f
        .request(
            Method::POST,
            format!("{}/entries", http.queue(&membership)),
            &host,
        )
        .json(&json!({"request_id":format!("{:032x}",0),"url":"https://youtu.be/dQw4w9WgXcQ"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(receipt["active"], false);
    assert_eq!(receipt["changed"], false);
}
