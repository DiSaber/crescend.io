use super::*;
use reqwest::Method;
use serde_json::{Value, json};

#[tokio::test]
async fn concurrent_votes_batch_policy_and_membership_failures_roll_back() {
    use crate::{database::queue::Moderation, models::queue::PolicyRequest};
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&host, &membership, 3).await;
    f.database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 99 }),
            || f.now(),
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        f.database.moderate_queue(
            &guest,
            &generation,
            Moderation::Vote(ids[0].as_str()),
            || f.now()
        ),
        f.database.moderate_queue(
            &guest,
            &generation,
            Moderation::Vote(ids[0].as_str()),
            || f.now()
        )
    );
    assert_ne!(a.unwrap().changed, b.unwrap().changed);
    f.database
        .moderate_queue(
            &guest,
            &generation,
            Moderation::Vote(ids[1].as_str()),
            || f.now(),
        )
        .await
        .unwrap();
    f.fail_revision().await;
    assert!(
        f.database
            .moderate_queue(
                &guest,
                &generation,
                Moderation::Vote(ids[2].as_str()),
                || f.now()
            )
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM queue_votes")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap(),
        2
    );
    assert!(
        f.database
            .moderate_queue(
                &host,
                &membership,
                Moderation::Policy(&PolicyRequest::Fixed { count: 1 }),
                || f.now()
            )
            .await
            .is_err()
    );
    let before = f
        .database
        .queue_view(&host, &membership, || f.now())
        .await
        .unwrap();
    assert_eq!(before.upcoming.len(), 2);
    assert_eq!(before.voting.fixed_count, Some(99));
    assert!(
        f.database
            .leave_lobby(guest.id, &generation, || f.now())
            .await
            .is_err()
    );
    let after = f
        .database
        .queue_view(&guest, &generation, || f.now())
        .await
        .unwrap();
    assert_eq!(after.revision, before.revision);
    assert!(after.current.unwrap().has_voted);
    sqlx::query("DROP TRIGGER fail_queue_revision")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let result = f
        .database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 1 }),
            || f.now(),
        )
        .await
        .unwrap();
    assert_eq!(result.queue.current.unwrap().id, ids[2]);
    assert!(result.queue.upcoming.is_empty());
    assert_eq!(result.queue.revision.value(), before.revision.value() + 1);
}

#[tokio::test]
async fn zero_guests_never_remove_and_one_guest_rejoin_meets_fixed_threshold() {
    use crate::{database::queue::Moderation, models::queue::PolicyRequest};
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    assert!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .voting
            .effective_threshold
            .is_none()
    );
    let (guest, generation) = f.guest("guest", &code).await;
    let ids = f.seed(&guest, &generation, 1).await;
    f.database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 99 }),
            || f.now(),
        )
        .await
        .unwrap();
    let voted = f
        .database
        .moderate_queue(
            &guest,
            &generation,
            Moderation::Vote(ids[0].as_str()),
            || f.now(),
        )
        .await
        .unwrap();
    assert!(voted.queue.current.unwrap().has_voted);
    assert!(
        !f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .current
            .unwrap()
            .has_voted
    );
    f.database
        .leave_lobby(guest.id, &generation, || f.now())
        .await
        .unwrap();
    let policy = f
        .database
        .moderate_queue(
            &host,
            &membership,
            Moderation::Policy(&PolicyRequest::Fixed { count: 1 }),
            || f.now(),
        )
        .await
        .unwrap();
    assert!(policy.queue.current.is_some());
    f.database
        .join_lobby(guest.id, &code, || f.now())
        .await
        .unwrap();
    assert!(
        f.database
            .queue_view(&host, &membership, || f.now())
            .await
            .unwrap()
            .current
            .is_none()
    );
}

#[tokio::test]
async fn majority_removes_once_and_fixed_policy_retains_private_account_votes() {
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let mut guests = Vec::new();
    for i in 0..3 {
        guests.push(f.guest(&format!("guest{i}"), &code).await);
    }
    let ids = f.seed(&host, &membership, 3).await;
    let http = f.server().await;
    let base = http.queue(&membership);
    let vote = |i: usize| {
        f.request(
            Method::POST,
            format!(
                "{}/entries/{}/votes",
                http.queue(&guests[i].1),
                ids[0].as_str()
            ),
            &guests[i].0,
        )
    };
    let first: Value = vote(0).send().await.unwrap().json().await.unwrap();
    assert_eq!(first["queue"]["current"]["active_vote_count"], 1);
    let repeat: Value = vote(0).send().await.unwrap().json().await.unwrap();
    assert_eq!(repeat["changed"], false);
    let second: Value = vote(1).send().await.unwrap().json().await.unwrap();
    assert_eq!(second["queue"]["current"]["id"], ids[1].as_str());
    assert_eq!(vote(0).send().await.unwrap().status(), 404);
    assert_eq!(
        f.request(Method::POST, format!("{base}/advance"), &host)
            .json(&json!({"expected_entry_id":ids[0].as_str(),"reason":"ended"}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let policy: Value = f
        .request(Method::PUT, format!("{base}/voting-policy"), &host)
        .json(&json!({"mode":"fixed","count":99}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(policy["queue"]["voting"]["effective_threshold"], 99);
    for (guest, generation) in &guests {
        assert_eq!(
            f.request(
                Method::POST,
                format!(
                    "{}/entries/{}/votes",
                    http.queue(generation),
                    ids[1].as_str()
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
    let lowered: Value = f
        .request(Method::PUT, format!("{base}/voting-policy"), &host)
        .json(&json!({"mode":"fixed","count":1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(lowered["queue"]["current"]["id"], ids[2].as_str());
    assert!(!lowered.to_string().contains("voter"));
}
