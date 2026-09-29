use super::us5_http::{leave, setup};
use super::*;

#[tokio::test]
async fn expired_and_closed_generations_are_noops_without_revision_changes() {
    for closed in [false, true] {
        let f = Fixture::new().await;
        let (owner, user, member) = setup(&f).await;
        if closed {
            sqlx::query("UPDATE lobbies SET closed_at = created_at")
                .execute(&f.database.db_pool)
                .await
                .unwrap();
        } else {
            f.time.store(NOW + 86_400, Ordering::SeqCst);
        }
        for (actor, view) in [(f.user, owner), (user, member)] {
            assert_eq!(
                leave(&f, actor, view["membership_id"].as_str().unwrap())
                    .await
                    .status(),
                204
            );
        }
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM lobbies")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
        assert_eq!(revision, 2);
        let closure: Option<String> = sqlx::query_scalar("SELECT closed_at FROM lobbies")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
        assert_eq!(closure.is_some(), closed);
    }
}

#[tokio::test]
async fn revision_failure_rolls_back_ordinary_deletion() {
    let f = Fixture::new().await;
    let (_, user, member) = setup(&f).await;
    sqlx::query("CREATE TRIGGER reject_revision BEFORE UPDATE ON lobbies BEGIN SELECT RAISE(ABORT, 'test'); END")
        .execute(&f.database.db_pool).await.unwrap();
    assert_eq!(
        leave(&f, user, member["membership_id"].as_str().unwrap())
            .await
            .status(),
        500
    );
    assert_eq!(
        serde_json::to_value(f.database.current_lobby(user, || NOW).await.unwrap()).unwrap(),
        member
    );
}

#[tokio::test]
async fn concurrent_join_and_owner_leave_never_leave_members_in_closed_lobby() {
    for _ in 0..8 {
        let f = Fixture::new().await;
        let (owner, user, _) = setup(&f).await;
        let newcomer = f.database.create_user("racing-joiner").await.unwrap().id;
        let code = owner["lobby"]["join_code"].as_str().unwrap();
        let (left, joined) = tokio::join!(
            leave(&f, f.user, owner["membership_id"].as_str().unwrap()),
            f.database.join_lobby(newcomer, code, || NOW)
        );
        assert_eq!(left.status(), 204);
        assert!(
            joined.is_ok()
                || matches!(
                    joined,
                    Err(crate::database::lobbies::LobbyError::LobbyUnavailable)
                )
        );
        assert_eq!(f.count("lobby_memberships").await, 0);
        let (revision, expires, closed, original_owner): (i64, String, String, i64) =
            sqlx::query_as("SELECT revision, expires_at, closed_at, owner_user_id FROM lobbies")
                .fetch_one(&f.database.db_pool)
                .await
                .unwrap();
        assert!(revision == 3 || revision == 4);
        assert_eq!(original_owner, f.user);
        assert!(expires > closed);
        for actor in [f.user, user, newcomer] {
            assert!(
                f.database
                    .current_lobby(actor, || NOW)
                    .await
                    .unwrap()
                    .lobby
                    .is_none()
            );
        }
    }
}

#[tokio::test]
async fn concurrent_duplicate_leave_changes_revision_once() {
    let f = Fixture::new().await;
    let (_, user, member) = setup(&f).await;
    let id = member["membership_id"].as_str().unwrap();
    let (a, b) = tokio::join!(leave(&f, user, id), leave(&f, user, id));
    assert_eq!(a.status(), 204);
    assert_eq!(b.status(), 204);
    let current = f
        .database
        .current_lobby(f.user, || NOW)
        .await
        .unwrap()
        .lobby
        .unwrap();
    assert_eq!(current.revision, "3");
    assert_eq!(current.members.len(), 1);
    let replacement = f.database.create_owned_lobby(user, || NOW).await.unwrap();
    assert_eq!(leave(&f, user, id).await.status(), 204);
    assert_eq!(
        serde_json::to_value(f.database.current_lobby(user, || NOW).await.unwrap()).unwrap(),
        serde_json::to_value(replacement).unwrap()
    );
}

#[tokio::test]
async fn leave_and_join_sample_expiry_after_waiting_for_writer() {
    let f = Fixture::new().await;
    let (owner, _, _) = setup(&f).await;
    let newcomer = f.database.create_user("deadline-joiner").await.unwrap().id;
    let tx = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    let sampled = Arc::new(AtomicI64::new(0));
    let left = {
        let database = f.database.clone();
        let time = f.time.clone();
        let sampled = sampled.clone();
        let user = f.user;
        let id = owner["membership_id"].as_str().unwrap().to_owned();
        tokio::spawn(async move {
            database
                .leave_lobby(user, &id, || {
                    sampled.fetch_add(1, Ordering::SeqCst);
                    time.load(Ordering::SeqCst)
                })
                .await
        })
    };
    let joined = {
        let database = f.database.clone();
        let time = f.time.clone();
        let code = owner["lobby"]["join_code"].as_str().unwrap().to_owned();
        tokio::spawn(async move {
            database
                .join_lobby(newcomer, &code, || time.load(Ordering::SeqCst))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(sampled.load(Ordering::SeqCst), 0);
    f.time.store(NOW + 86_400, Ordering::SeqCst);
    tx.commit().await.unwrap();
    assert!(left.await.unwrap().unwrap().is_none());
    assert!(matches!(
        joined.await.unwrap(),
        Err(crate::database::lobbies::LobbyError::LobbyUnavailable)
    ));
    let (revision, closed): (i64, Option<String>) =
        sqlx::query_as("SELECT revision, closed_at FROM lobbies")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
    assert_eq!(revision, 2);
    assert!(closed.is_none());
    assert_eq!(sampled.load(Ordering::SeqCst), 1);
}
