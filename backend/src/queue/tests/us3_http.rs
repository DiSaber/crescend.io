use super::*;
use reqwest::Method;
use serde_json::Value;

#[tokio::test]
async fn only_host_vetoes_an_occurrence_and_retry_is_noop() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let entries = f.seed(&host, &membership, 3).await;
    let http = f.server().await;
    let suffix = format!("/entries/{}", entries[0].as_str());
    assert_eq!(
        f.request(
            Method::DELETE,
            format!("{}{suffix}", http.queue(&generation)),
            &guest
        )
        .send()
        .await
        .unwrap()
        .status(),
        403
    );
    let response = f
        .request(
            Method::DELETE,
            format!("{}{suffix}", http.queue(&membership)),
            &host,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "host veto must be available");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["changed"], true);
    assert_eq!(result["queue"]["current"]["id"], entries[1].as_str());
    let result: Value = f
        .request(
            Method::DELETE,
            format!("{}{suffix}", http.queue(&membership)),
            &host,
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["changed"], false);
    assert_eq!(
        f.request(
            Method::DELETE,
            format!("{}{suffix}", http.queue(&membership)),
            &host
        )
        .body(" ")
        .send()
        .await
        .unwrap()
        .status(),
        400
    );
}
