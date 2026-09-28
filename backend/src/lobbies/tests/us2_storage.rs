use super::*;
use crate::{database::lobbies::LobbyError, models::lobby::MembershipId};

#[tokio::test]
async fn simultaneous_first_joins_share_one_generation_and_survive_restart() {
    let f = Fixture::new().await;
    let target = f
        .database
        .create_owned_lobby(f.user, || NOW)
        .await
        .unwrap()
        .lobby
        .unwrap();
    let user = f.database.create_user("racer").await.unwrap().id;
    let (a, b) = tokio::join!(
        f.database.join_lobby(user, &target.join_code, || NOW),
        f.database.join_lobby(user, &target.join_code, || NOW)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.membership_id, b.membership_id);
    assert_eq!(a.lobby.unwrap().revision, "2");
    assert_eq!(b.lobby.unwrap().revision, "2");
    let reopened = Fixture::connect(&f.directory.path().join("test.db")).await;
    let retry = reopened
        .join_lobby_with(
            user,
            &target.join_code,
            || NOW + 1,
            || panic!("repeat must not allocate"),
        )
        .await
        .unwrap();
    assert_eq!(retry.membership_id, a.membership_id);
    assert_eq!(retry.lobby.unwrap().revision, "2");
    reopened.db_pool.close().await;
}

#[tokio::test]
async fn failed_commit_does_not_leave_a_membership_or_revision_change() {
    let f = Fixture::new().await;
    let target = f
        .database
        .create_owned_lobby(f.user, || NOW)
        .await
        .unwrap()
        .lobby
        .unwrap();
    let user = f.database.create_user("joiner").await.unwrap().id;
    sqlx::query("CREATE TABLE deferred_join (user_id INTEGER REFERENCES users(id) DEFERRABLE INITIALLY DEFERRED)").execute(&f.database.db_pool).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_join_commit AFTER INSERT ON lobby_memberships BEGIN INSERT INTO deferred_join VALUES (999999); END").execute(&f.database.db_pool).await.unwrap();
    assert!(matches!(
        f.database.join_lobby(user, &target.join_code, || NOW).await,
        Err(LobbyError::Database(_))
    ));
    assert_eq!(f.count("lobby_memberships").await, 1);
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM lobbies")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(revision, 1);
}

#[tokio::test]
async fn writer_wait_samples_time_before_target_availability() {
    let f = Fixture::new().await;
    let target = f
        .database
        .create_owned_lobby(f.user, || NOW)
        .await
        .unwrap()
        .lobby
        .unwrap();
    let user = f.database.create_user("joiner").await.unwrap().id;
    let tx = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    let db = f.database.clone();
    let clock = f.time.clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(async move {
        started.send(()).unwrap();
        db.join_lobby(user, &target.join_code, || clock.load(Ordering::SeqCst))
            .await
    });
    waiting.await.unwrap();
    f.time.store(NOW + 86400, Ordering::SeqCst);
    tx.rollback().await.unwrap();
    assert!(matches!(
        request.await.unwrap(),
        Err(LobbyError::LobbyUnavailable)
    ));
    assert_eq!(f.count("lobby_memberships").await, 1);
}

#[tokio::test]
async fn concurrent_joins_preserve_members_owner_and_retry_generation() {
    let f = Fixture::new().await;
    let lobby = f
        .database
        .create_owned_lobby(f.user, || NOW)
        .await
        .unwrap()
        .lobby
        .unwrap();
    let a = f.database.create_user("a").await.unwrap().id;
    let b = f.database.create_user("b").await.unwrap().id;
    let other = Fixture::connect(&f.directory.path().join("test.db")).await;
    let (x, y) = tokio::join!(
        f.database.join_lobby(a, &lobby.join_code, || NOW),
        other.join_lobby(b, &lobby.join_code, || NOW)
    );
    x.unwrap();
    y.unwrap();
    let (x, y) = tokio::join!(
        f.database.join_lobby(a, &lobby.join_code, || NOW + 1),
        other.join_lobby(a, &lobby.join_code, || NOW + 1)
    );
    let x = x.unwrap();
    let y = y.unwrap();
    assert_eq!(x.membership_id, y.membership_id);
    let view = x.lobby.unwrap();
    assert_eq!(view.revision, "3");
    assert_eq!(view.members.len(), 3);
    assert_eq!(view.owner_user_id, f.user.to_string());
    assert_eq!(view.expires_at, lobby.expires_at);
    other.db_pool.close().await;
}

#[tokio::test]
async fn competing_targets_and_create_join_have_one_winner() {
    for create in [false, true] {
        let f = Fixture::new().await;
        let target = f
            .database
            .create_owned_lobby(f.user, || NOW)
            .await
            .unwrap()
            .lobby
            .unwrap();
        let owner = f.database.create_user("owner2").await.unwrap().id;
        let target2 = f
            .database
            .create_owned_lobby(owner, || NOW)
            .await
            .unwrap()
            .lobby
            .unwrap();
        let user = f.database.create_user("racer").await.unwrap().id;
        let (x, y) = tokio::join!(
            f.database.join_lobby(user, &target.join_code, || NOW),
            async {
                if create {
                    f.database.create_owned_lobby(user, || NOW).await
                } else {
                    f.database
                        .join_lobby(user, &target2.join_code, || NOW)
                        .await
                }
            }
        );
        assert_eq!(usize::from(x.is_ok()) + usize::from(y.is_ok()), 1);
        assert!(matches!(
            x.err().or(y.err()).unwrap(),
            LobbyError::AlreadyInLobby
        ));
        assert_eq!(f.count("lobby_memberships").await, 3);
    }
}

#[tokio::test]
async fn collision_exhaustion_and_failed_revision_restore_stale_slot() {
    let f = Fixture::new().await;
    let stale = f.database.create_owned_lobby(f.user, || NOW).await.unwrap();
    let owner = f.database.create_user("target-owner").await.unwrap().id;
    let target = f
        .database
        .create_owned_lobby(owner, || NOW + 1)
        .await
        .unwrap();
    let code = &target.lobby.as_ref().unwrap().join_code;
    let collision = target.membership_id.unwrap();
    let mut attempts = 0;
    let result = f
        .database
        .join_lobby_with(
            f.user,
            code,
            || NOW + 86400,
            || {
                attempts += 1;
                Ok(collision.clone())
            },
        )
        .await;
    assert!(matches!(result, Err(LobbyError::Unavailable)));
    assert_eq!(attempts, 8);
    assert!(matches!(
        f.database
            .join_lobby_with(
                f.user,
                code,
                || NOW + 86400,
                || Err(LobbyError::Unavailable)
            )
            .await,
        Err(LobbyError::Unavailable)
    ));
    sqlx::query("CREATE TRIGGER fail_revision BEFORE UPDATE OF revision ON lobbies BEGIN SELECT RAISE(ABORT, 'private failure'); END").execute(&f.database.db_pool).await.unwrap();
    assert!(matches!(
        f.database.join_lobby(f.user, code, || NOW + 86400).await,
        Err(LobbyError::Database(_))
    ));
    let id: MembershipId = sqlx::query_scalar("SELECT id FROM lobby_memberships WHERE user_id = ?")
        .bind(f.user)
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(Some(id), stale.membership_id);
    sqlx::query("DROP TRIGGER fail_revision")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let mut attempts = 0;
    let view = f
        .database
        .join_lobby_with(
            f.user,
            code,
            || NOW + 86400,
            || {
                attempts += 1;
                Ok(if attempts == 1 {
                    collision.clone()
                } else {
                    MembershipId::from_bytes([42; 16])
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(view.lobby.unwrap().revision, "2");
    assert_eq!(f.count("lobby_memberships").await, 2);
}

#[tokio::test]
async fn closed_slots_release_and_nonexistent_accounts_cannot_join() {
    let f = Fixture::new().await;
    f.database.create_owned_lobby(f.user, || NOW).await.unwrap();
    sqlx::query("UPDATE lobbies SET closed_at = created_at")
        .execute(&f.database.db_pool)
        .await
        .unwrap();
    let owner = f.database.create_user("target").await.unwrap().id;
    let target = f
        .database
        .create_owned_lobby(owner, || NOW)
        .await
        .unwrap()
        .lobby
        .unwrap();
    assert!(matches!(
        f.database
            .join_lobby(999999, &target.join_code, || NOW)
            .await,
        Err(LobbyError::Unauthorized)
    ));
    let view = f
        .database
        .join_lobby(f.user, &target.join_code, || NOW)
        .await
        .unwrap();
    assert_eq!(view.lobby.unwrap().revision, "2");
    assert_eq!(f.count("lobby_memberships").await, 2);
}
