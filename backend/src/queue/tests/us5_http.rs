use super::*;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn policies_are_strict_and_roles_precede_targets() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let http = f.server().await;
    let url = http.queue(&membership);
    for body in [
        r#"{"mode":"majority","mode":"fixed","count":1}"#,
        r#"{"mode":"fixed","count":1,"count":2}"#,
        r#"{"mode":"majority","count":null}"#,
        r#"{"mode":"majority","extra":true}"#,
    ] {
        assert_eq!(
            f.request(Method::PUT, format!("{url}/voting-policy"), &host)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .unwrap()
                .status(),
            400,
            "{body}"
        );
    }
    for body in [
        json!({"mode":"majority","count":1}),
        json!({"mode":"fixed"}),
        json!({"mode":"fixed","count":null}),
        json!({"mode":"fixed","count":0}),
        json!({"mode":"fixed","count":-1}),
        json!({"mode":"fixed","count":1.5}),
        json!({"mode":"fixed","count":2147483648_i64}),
    ] {
        assert_eq!(
            f.request(Method::PUT, format!("{url}/voting-policy"), &host)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(
        f.request(
            Method::PUT,
            format!("{}/voting-policy", http.queue(&generation)),
            &guest
        )
        .json(&json!({"mode":"majority"}))
        .send()
        .await
        .unwrap()
        .status(),
        403
    );
    assert_eq!(
        f.request(
            Method::POST,
            format!("{url}/entries/{:032x}/votes", 99),
            &host
        )
        .send()
        .await
        .unwrap()
        .status(),
        403
    );
    assert_eq!(
        f.request(
            Method::POST,
            format!("{}/entries/{:032x}/votes", http.queue(&generation), 99),
            &guest
        )
        .send()
        .await
        .unwrap()
        .status(),
        404
    );
}
