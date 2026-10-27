use super::*;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn duplicate_veto_append_race_and_last_removal_preserve_survivors() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    let entries = f.seed(&host, &membership, 3).await;
    f.mock_metadata().await;
    let http = f.server().await;
    let url = http.queue(&membership);
    let target = format!("{url}/entries/{}", entries[0].as_str());
    let (a, b, c) = tokio::join!(
        f.request(Method::DELETE, target.clone(), &host).send(),
        f.request(Method::DELETE, target, &host).send(),
        f.request(Method::POST, format!("{url}/entries"), &host)
            .json(&json!({"url":"https://youtu.be/dQw4w9WgXcQ","request_id":format!("{:032x}",99)}))
            .send()
    );
    let a: Value = a.unwrap().json().await.unwrap();
    let b: Value = b.unwrap().json().await.unwrap();
    assert_ne!(a["changed"], b["changed"]);
    assert_eq!(c.unwrap().status(), 200);
    let view = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(view.revision.value(), 6);
    assert_eq!(view.current.as_ref().unwrap().id, entries[1]);
    assert_eq!(view.upcoming[0].id, entries[2]);
    for entry in view.current.iter().chain(view.upcoming.iter()) {
        assert_eq!(
            f.request(
                Method::DELETE,
                format!("{url}/entries/{}", entry.id.as_str()),
                &host
            )
            .send()
            .await
            .unwrap()
            .status(),
            200
        );
    }
    let view = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert!(view.current.is_none());
    assert!(view.upcoming.is_empty());
    assert!(
        f.database
            .current_lobby(host.id, || f.now())
            .await
            .unwrap()
            .lobby
            .is_some()
    );
}

#[tokio::test]
async fn veto_retains_receipt_and_failure_restores_order() {
    let f = Fixture::new().await;
    let (host, membership, _) = f.host().await;
    let entries = f.seed(&host, &membership, 3).await;
    let http = f.server().await;
    let url = http.queue(&membership);
    let response = f
        .request(
            Method::DELETE,
            format!("{url}/entries/{}", entries[1].as_str()),
            &host,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let retry: Value = f
        .request(Method::POST, format!("{url}/entries"), &host)
        .json(&json!({"url":"https://youtu.be/dQw4w9WgXcQ", "request_id":format!("{:032x}",1)}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retry["active"], false);
    assert_eq!(retry["changed"], false);
    assert_eq!(retry["queue"]["upcoming"][0]["id"], entries[2].as_str());
    f.fail_revision().await;
    assert_eq!(
        f.request(
            Method::DELETE,
            format!("{url}/entries/{}", entries[0].as_str()),
            &host
        )
        .send()
        .await
        .unwrap()
        .status(),
        500
    );
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .current
            .unwrap()
            .id,
        entries[0]
    );
}
