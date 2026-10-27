use super::*;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn first_last_moves_preserve_occurrences_votes_and_survivors() {
    use crate::{database::queue::Moderation, models::queue::PolicyRequest};
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&host, &membership, 5).await;
    f.database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 99 }),
            || f.now(),
        )
        .await
        .unwrap();
    f.database
        .moderate_queue(
            &guest,
            &generation,
            Moderation::Vote(ids[4].as_str()),
            || f.now(),
        )
        .await
        .unwrap();
    let initial = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    let expected = serde_json::to_value(&initial.upcoming[3]).unwrap();
    let http = f.server().await;
    let base = http.queue(&membership);
    let url = format!("{base}/entries/{}/move", ids[4].as_str());
    let body = json!({"expected_revision":initial.revision.value().to_string(),"to_index":0});
    let response = f
        .request(Method::POST, url.clone(), &host)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "host move must be available");
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["queue"]["current"]["id"], ids[0].as_str());
    assert_eq!(result["queue"]["upcoming"][0], expected);
    assert_eq!(result["queue"]["upcoming"][1]["id"], ids[1].as_str());
    assert_eq!(
        f.request(Method::POST, url.clone(), &host)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let result: Value = f
        .request(Method::POST, url, &host)
        .json(&json!({"expected_revision":result["queue"]["revision"],"to_index":3}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["queue"]["upcoming"][3], expected);
    for (entry, id) in result["queue"]["upcoming"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&ids[1..])
    {
        assert_eq!(entry["id"], id.as_str());
    }
}

#[tokio::test]
async fn concurrent_append_and_membership_changes_reject_stale_move_intent() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let ids = f.seed(&host, &membership, 4).await;
    f.mock_metadata().await;
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
    let url = format!("{base}/entries/{}/move", ids[3].as_str());
    let (moved, appended) = tokio::join!(
        f.request(Method::POST, url.clone(), &host)
            .json(&json!({"expected_revision":revision,"to_index":0}))
            .send(),
        f.request(Method::POST, format!("{base}/entries"), &host)
            .json(&json!({"url":"https://youtu.be/dQw4w9WgXcQ","request_id":format!("{:032x}",99)}))
            .send()
    );
    assert!([200, 409].contains(&moved.unwrap().status().as_u16()));
    assert_eq!(appended.unwrap().status(), 200);
    let before = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(before.upcoming.len(), 4);
    f.guest("new-guest", &code).await;
    let response = f
        .request(Method::POST, url, &host)
        .json(&json!({"expected_revision":before.revision.value().to_string(),"to_index":0}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "stale_queue_revision"
    );
}
