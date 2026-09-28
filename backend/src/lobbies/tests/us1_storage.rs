use super::*;
use crate::{
    database::lobbies::LobbyError,
    models::lobby::{LobbyAllocation, LobbyId, MembershipId},
};

fn allocation(n: u8) -> LobbyAllocation {
    LobbyAllocation {
        lobby_id: LobbyId::from_bytes([n; 16]),
        membership_id: MembershipId::from_bytes([n.wrapping_add(100); 16]),
        join_code: format!("AB{n:04}"),
    }
}

async fn create(f: &Fixture, user: i64, n: u8) -> crate::models::lobby::CurrentLobby {
    f.database
        .create_owned_lobby_with(user, || f.time.load(Ordering::SeqCst), || Ok(allocation(n)))
        .await
        .unwrap()
}

#[tokio::test]
async fn initialization_constraints_and_restart_preserve_state() {
    let f = Fixture::new().await;
    let created = create(&f, f.user, 1).await;
    f.database.create_tables().await.unwrap();
    let reopened = Fixture::connect(&f.directory.path().join("test.db")).await;
    reopened.create_tables().await.unwrap();
    let row: (
        String,
        i64,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
        i64,
    ) = sqlx::query_as("SELECT id, owner_user_id, created_at, expires_at, revision FROM lobbies")
        .fetch_one(&reopened.db_pool)
        .await
        .unwrap();
    assert_eq!(row.0, created.lobby.unwrap().id.as_str());
    assert_eq!(row.1, f.user);
    assert_eq!(row.4, 1);
    assert_eq!((row.3 - row.2).num_seconds(), 86_400);
    let joined: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT joined_at FROM lobby_memberships")
            .fetch_one(&reopened.db_pool)
            .await
            .unwrap();
    let closed: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT closed_at FROM lobbies")
            .fetch_one(&reopened.db_pool)
            .await
            .unwrap();
    assert_eq!(joined, row.2);
    assert_eq!(closed, None);
    let mut connections = Vec::new();
    for _ in 0..4 {
        connections.push(reopened.db_pool.acquire().await.unwrap());
    }
    for connection in &mut connections {
        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        assert_eq!(enabled, 1);
    }
    drop(connections);
    assert!(
        sqlx::query("UPDATE lobbies SET join_code = 'bad' ")
            .execute(&reopened.db_pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE lobbies SET revision = 0")
            .execute(&reopened.db_pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE lobbies SET id = 'NOT-HEX'")
            .execute(&reopened.db_pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE lobbies SET expires_at = created_at")
            .execute(&reopened.db_pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE lobbies SET owner_user_id = 99999")
            .execute(&reopened.db_pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE lobby_memberships SET lobby_id = 'ffffffffffffffffffffffffffffffff'")
            .execute(&reopened.db_pool)
            .await
            .is_err()
    );
    assert!(matches!(
        reopened
            .create_owned_lobby_with(f.user, || NOW, || Ok(allocation(2)))
            .await,
        Err(LobbyError::AlreadyInLobby)
    ));
    reopened.db_pool.close().await;
}

#[tokio::test]
async fn concurrent_creates_across_pools_have_one_winner() {
    let f = Fixture::new().await;
    let other = Fixture::connect(&f.directory.path().join("test.db")).await;
    let (a, b) = tokio::join!(
        f.database
            .create_owned_lobby_with(f.user, || NOW, || Ok(allocation(1))),
        other.create_owned_lobby_with(f.user, || NOW, || Ok(allocation(2))),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let error = if a.is_err() {
        a.unwrap_err()
    } else {
        b.unwrap_err()
    };
    assert!(matches!(error, LobbyError::AlreadyInLobby));
    assert_eq!(f.count("lobbies").await, 1);
    assert_eq!(f.count("lobby_memberships").await, 1);
    other.db_pool.close().await;
}

#[tokio::test]
async fn code_and_both_identity_collisions_retry_without_orphans() {
    for collision in 0..3 {
        let f = Fixture::new().await;
        create(&f, f.user, 1).await;
        let user = f.database.create_user("second").await.unwrap().id;
        let mut attempts = 0;
        let result = f
            .database
            .create_owned_lobby_with(
                user,
                || NOW,
                || {
                    attempts += 1;
                    let mut value = allocation(2);
                    if attempts == 1 {
                        match collision {
                            0 => value.join_code = allocation(1).join_code,
                            1 => value.lobby_id = allocation(1).lobby_id,
                            _ => value.membership_id = allocation(1).membership_id,
                        }
                    }
                    Ok(value)
                },
            )
            .await
            .unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(result.lobby.unwrap().owner_user_id, user.to_string());
        assert_eq!(f.count("lobbies").await, 2);
        assert_eq!(f.count("lobby_memberships").await, 2);
    }
}

#[tokio::test]
async fn exhausted_collisions_and_random_failure_leave_no_partial_state() {
    let f = Fixture::new().await;
    create(&f, f.user, 1).await;
    let user = f.database.create_user("second").await.unwrap().id;
    let mut attempts = 0;
    let result = f
        .database
        .create_owned_lobby_with(
            user,
            || NOW,
            || {
                attempts += 1;
                let mut value = allocation(2);
                value.membership_id = allocation(1).membership_id;
                Ok(value)
            },
        )
        .await;
    assert!(matches!(result, Err(LobbyError::Unavailable)));
    assert_eq!(attempts, 8);
    assert!(matches!(
        f.database
            .create_owned_lobby_with(user, || NOW, || Err(LobbyError::Unavailable))
            .await,
        Err(LobbyError::Unavailable)
    ));
    assert_eq!(f.count("lobbies").await, 1);
    assert_eq!(f.count("lobby_memberships").await, 1);
}

#[tokio::test]
async fn failed_member_insert_rolls_back_lobby_and_is_not_retried() {
    let f = Fixture::new().await;
    sqlx::query("CREATE TRIGGER fail_member BEFORE INSERT ON lobby_memberships BEGIN SELECT RAISE(ABORT, 'member insert failure'); END")
        .execute(&f.database.db_pool).await.unwrap();
    let mut attempts = 0;
    let result = f
        .database
        .create_owned_lobby_with(
            f.user,
            || NOW,
            || {
                attempts += 1;
                Ok(allocation(1))
            },
        )
        .await;
    assert!(matches!(result, Err(LobbyError::Database(_))));
    assert_eq!(attempts, 1);
    assert_eq!(f.count("lobbies").await, 0);
    assert_eq!(f.count("lobby_memberships").await, 0);
}

#[tokio::test]
async fn logical_expiry_and_closed_lobbies_release_membership_before_cleanup() {
    for closed in [false, true] {
        let f = Fixture::new().await;
        create(&f, f.user, 1).await;
        if closed {
            sqlx::query("UPDATE lobbies SET closed_at = created_at")
                .execute(&f.database.db_pool)
                .await
                .unwrap();
        } else {
            f.time.store(NOW + 86_400 - 1, Ordering::SeqCst);
            assert!(matches!(
                f.database
                    .create_owned_lobby_with(
                        f.user,
                        || f.time.load(Ordering::SeqCst),
                        || Ok(allocation(2))
                    )
                    .await,
                Err(LobbyError::AlreadyInLobby)
            ));
            f.time.store(NOW + 86_400, Ordering::SeqCst);
        }
        let second = create(&f, f.user, 2).await;
        assert_eq!(second.membership_id.unwrap(), allocation(2).membership_id);
        assert_eq!(f.count("lobbies").await, 2);
        assert_eq!(f.count("lobby_memberships").await, 1);
    }
}

#[tokio::test]
async fn cleanup_cascades_expired_memberships_only_and_preserves_auth() {
    let f = Fixture::new().await;
    create(&f, f.user, 1).await;
    f.database
        .create_refresh_session(f.user, NOW, NOW + 2_592_000, &[7; 32])
        .await
        .unwrap();
    let user = f.database.create_user("second").await.unwrap().id;
    f.time.store(NOW + 1, Ordering::SeqCst);
    create(&f, user, 2).await;
    let before_deadline = chrono::DateTime::from_timestamp(NOW + 86_399, 999_999_999).unwrap();
    assert_eq!(
        f.database
            .delete_expired_lobbies(before_deadline)
            .await
            .unwrap(),
        0
    );
    let count = f
        .database
        .delete_expired_lobbies(chrono::DateTime::from_timestamp(NOW + 86_400, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(f.count("lobbies").await, 1);
    assert_eq!(f.count("lobby_memberships").await, 1);
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_sessions")
        .fetch_one(&f.database.db_pool)
        .await
        .unwrap();
    assert_eq!(sessions, 1);
    assert!(
        f.database
            .get_user_by_google_sub("private-provider-subject")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn failed_creation_restores_a_reclaimed_stale_membership() {
    let f = Fixture::new().await;
    create(&f, f.user, 1).await;
    let result = f
        .database
        .create_owned_lobby_with(f.user, || NOW + 86_400, || Err(LobbyError::Unavailable))
        .await;
    assert!(matches!(result, Err(LobbyError::Unavailable)));
    assert_eq!(f.count("lobbies").await, 1);
    assert_eq!(f.count("lobby_memberships").await, 1);
}

#[tokio::test]
async fn account_validation_precedes_allocation() {
    let f = Fixture::new().await;
    let result = f
        .database
        .create_owned_lobby_with(999999, || NOW, || panic!("must not allocate"))
        .await;
    assert!(matches!(result, Err(LobbyError::Unauthorized)));
    assert_eq!(f.count("lobbies").await, 0);
}

#[tokio::test]
async fn commit_failure_rolls_back_both_records() {
    let f = Fixture::new().await;
    sqlx::query("CREATE TABLE deferred_check (user_id INTEGER REFERENCES users(id) DEFERRABLE INITIALLY DEFERRED)")
        .execute(&f.database.db_pool).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_commit AFTER INSERT ON lobby_memberships BEGIN INSERT INTO deferred_check VALUES (999999); END")
        .execute(&f.database.db_pool).await.unwrap();
    assert!(matches!(
        f.database
            .create_owned_lobby_with(f.user, || NOW, || Ok(allocation(1)))
            .await,
        Err(LobbyError::Database(_))
    ));
    assert_eq!(f.count("lobbies").await, 0);
    assert_eq!(f.count("lobby_memberships").await, 0);
}

#[tokio::test]
async fn current_time_is_sampled_after_waiting_for_the_writer() {
    let f = Fixture::new().await;
    let tx = f
        .database
        .db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    let db = f.database.clone();
    let clock = f.time.clone();
    let user = f.user;
    let (started, waiting) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(async move {
        started.send(()).unwrap();
        db.create_owned_lobby_with(user, || clock.load(Ordering::SeqCst), || Ok(allocation(1)))
            .await
            .unwrap()
    });
    waiting.await.unwrap();
    f.time.store(NOW + 100, Ordering::SeqCst);
    tx.rollback().await.unwrap();
    let lobby = request.await.unwrap().lobby.unwrap();
    assert_eq!(lobby.created_at.timestamp(), NOW + 100);
}
