use super::*;
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn joining_reactivates_a_qualifying_vote_inside_membership_commit() {
    use crate::{database::queue::Moderation, models::queue::PolicyRequest};
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let mut guests = Vec::new();
    for i in 0..3 {
        guests.push(f.guest(&format!("guest{i}"), &code).await);
    }
    let ids = f.seed(&host, &membership, 2).await;
    f.database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 99 }),
            || f.now(),
        )
        .await
        .unwrap();
    for (guest, generation) in &guests[..2] {
        f.database
            .moderate_queue(guest, generation, Moderation::Vote(ids[0].as_str()), || {
                f.now()
            })
            .await
            .unwrap();
    }
    f.database
        .leave_lobby(guests[0].0.id, &guests[0].1, || f.now())
        .await
        .unwrap();
    let policy = f
        .database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Majority {}),
            || f.now(),
        )
        .await
        .unwrap();
    assert_eq!(policy.queue.current.unwrap().id, ids[0]);
    let result = f
        .database
        .join_lobby(guests[0].0.id, &code, || f.now())
        .await
        .unwrap();
    let current = f
        .database
        .queue_view(&guests[0].0, result.membership_id.unwrap().as_str(), || {
            f.now()
        })
        .await
        .unwrap();
    assert_eq!(current.current.unwrap().id, ids[1]);
    assert_eq!(current.revision.value(), policy.queue.revision.value() + 1);
}

#[tokio::test]
async fn departure_rechecks_majority_and_rejoin_restores_one_vote() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let mut guests = Vec::new();
    for i in 0..4 {
        guests.push(f.guest(&format!("guest{i}"), &code).await);
    }
    let ids = f.seed(&host, &membership, 2).await;
    let http = f.server().await;
    for (guest, generation) in &guests[..2] {
        assert_eq!(
            f.request(
                Method::POST,
                format!(
                    "{}/entries/{}/votes",
                    http.queue(generation),
                    ids[0].as_str()
                ),
                guest
            )
            .send()
            .await
            .unwrap()
            .status(),
            200
        );
    }
    let before = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap()
        .revision
        .value();
    f.database
        .leave_lobby(guests[3].0.id, &guests[3].1, || f.now())
        .await
        .unwrap();
    let view = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(view.current.unwrap().id, ids[1]);
    assert_eq!(view.revision.value(), before + 1);
    f.request(
        Method::PUT,
        format!("{}/voting-policy", http.queue(&membership)),
        &host,
    )
    .json(&json!({"mode":"fixed","count":99}))
    .send()
    .await
    .unwrap();
    let (guest, generation) = &guests[0];
    f.request(
        Method::POST,
        format!(
            "{}/entries/{}/votes",
            http.queue(generation),
            ids[1].as_str()
        ),
        guest,
    )
    .send()
    .await
    .unwrap();
    f.database
        .leave_lobby(guest.id, generation, || f.now())
        .await
        .unwrap();
    assert_eq!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .current
            .unwrap()
            .active_vote_count,
        0
    );
    let rejoined = f
        .database
        .join_lobby(guest.id, &code, || f.now())
        .await
        .unwrap();
    let new = rejoined.membership_id.unwrap();
    let view = f
        .database
        .queue_view(guest, new.as_str(), || f.now())
        .await
        .unwrap();
    assert_eq!(view.current.unwrap().active_vote_count, 1);
    let response: serde_json::Value = f
        .request(
            Method::POST,
            format!(
                "{}/entries/{}/votes",
                http.queue(new.as_str()),
                ids[1].as_str()
            ),
            guest,
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["changed"], false);
}
