use super::*;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn moves_validate_identity_revision_index_and_role_in_order() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&host, &membership, 3).await;
    let http = f.server().await;
    let base = http.queue(&membership);
    let revision = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap()
        .revision
        .value()
        .to_string();
    let url = format!("{base}/entries/{}/move", ids[1].as_str());
    for body in [
        json!({"expected_revision":0,"to_index":0}),
        json!({"expected_revision":"0","to_index":0}),
        json!({"expected_revision":"01","to_index":0}),
        json!({"expected_revision":"9223372036854775808","to_index":0}),
        json!({"expected_revision":revision,"to_index":-1}),
        json!({"expected_revision":revision,"to_index":1.5}),
        json!({"expected_revision":revision,"to_index":2}),
        json!({"expected_revision":revision,"to_index":0,"extra":1}),
    ] {
        assert_eq!(
            f.request(Method::POST, url.clone(), &host)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            400,
            "{body}"
        );
    }
    let response: Value = f
        .request(Method::POST, url.clone(), &host)
        .json(&json!({"expected_revision":revision,"to_index":0}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["changed"], false);
    assert_eq!(response["queue"]["revision"], revision);
    for (target, expected, status, code) in [
        (
            ids[0].as_str(),
            revision.as_str(),
            409,
            "entry_not_upcoming",
        ),
        (
            "00000000000000000000000000000000",
            revision.as_str(),
            404,
            "queue_entry_unavailable",
        ),
        (
            "00000000000000000000000000000000",
            "1",
            409,
            "stale_queue_revision",
        ),
    ] {
        let response = f
            .request(Method::POST, format!("{base}/entries/{target}/move"), &host)
            .json(&json!({"expected_revision":expected,"to_index":0}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.json::<Value>().await.unwrap()["code"], code);
    }
    assert_eq!(
        f.request(
            Method::POST,
            format!(
                "{}/entries/{}/move",
                http.queue(&generation),
                ids[1].as_str()
            ),
            &guest
        )
        .json(&json!({"expected_revision":"1","to_index":0}))
        .send()
        .await
        .unwrap()
        .status(),
        403
    );
}
