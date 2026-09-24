use super::{AuthError, Clock, JwtAuth, TOKEN_SECONDS};
use crate::database::{Database, RefreshSessionError};
use rand::TryRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

pub const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

// Internal bundles deliberately do not implement Serialize or ToSchema.
pub struct RefreshCredential {
    pub refresh_token: String,
    pub expires_at: i64,
}

pub struct Rotation {
    pub access: TokenResponse,
    pub credential: RefreshCredential,
}

fn credential(expires_at: i64) -> Result<RefreshCredential, AuthError> {
    let mut random = [0u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut random)
        .map_err(|_| AuthError::Internal)?;
    Ok(RefreshCredential {
        refresh_token: base16ct::lower::encode_string(&random),
        expires_at,
    })
}

pub async fn issue(
    database: &Database,
    user: i64,
    clock: &Clock,
) -> Result<RefreshCredential, AuthError> {
    let now = clock();
    let expires = now
        .checked_add(SESSION_SECONDS)
        .ok_or(AuthError::Internal)?;
    let credential = credential(expires)?;
    database
        .create_refresh_session(user, now, expires, &hash(&credential.refresh_token))
        .await
        .map_err(|_| AuthError::Internal)?;
    Ok(credential)
}

pub async fn rotate(
    database: &Database,
    jwt: &JwtAuth,
    token: &str,
    clock: &Clock,
) -> Result<Rotation, AuthError> {
    if token.is_empty() {
        return Err(AuthError::Unauthorized);
    }
    database
        .rotate_refresh_session(
            &hash(token),
            || clock(),
            |user, expires_at| {
                let credential =
                    credential(expires_at).map_err(|_| RefreshSessionError::Internal)?;
                let access = TokenResponse {
                    access_token: jwt.issue(user).map_err(|_| RefreshSessionError::Internal)?,
                    token_type: "Bearer".into(),
                    expires_in: TOKEN_SECONDS,
                };
                let replacement_hash = hash(&credential.refresh_token);
                Ok((Rotation { access, credential }, replacement_hash))
            },
        )
        .await
        .map_err(|error| match error {
            RefreshSessionError::InvalidCredential => AuthError::Unauthorized,
            RefreshSessionError::Internal => AuthError::Internal,
        })
}

pub async fn revoke(database: &Database, token: &str) -> Result<(), AuthError> {
    database
        .revoke_refresh_session(&hash(token))
        .await
        .map_err(|_| AuthError::Internal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    struct Fixture {
        database: Database,
        jwt: JwtAuth,
        clock: Clock,
        time: Arc<AtomicI64>,
        user: i64,
        file: tempfile::TempDir,
    }
    impl Fixture {
        async fn new() -> Self {
            let file = tempfile::tempdir().unwrap();
            let database = Self::connect(&file).await;
            database.create_tables().await.unwrap();
            let user = database.create_user("google-subject").await.unwrap().id;
            let time = Arc::new(AtomicI64::new(1000));
            let t = time.clone();
            let clock: Clock = Arc::new(move || t.load(Ordering::SeqCst));
            let jwt = JwtAuth::new(b"test-only-secret-for-refresh-session-tests", clock.clone());
            Self {
                database,
                jwt,
                clock,
                time,
                user,
                file,
            }
        }
        async fn connect(file: &tempfile::TempDir) -> Database {
            let opts = SqliteConnectOptions::new()
                .filename(file.path().join("refresh.db"))
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5));
            Database {
                db_pool: SqlitePoolOptions::new()
                    .max_connections(4)
                    .connect_with(opts)
                    .await
                    .unwrap(),
            }
        }
        async fn issue(&self) -> RefreshCredential {
            issue(&self.database, self.user, &self.clock).await.unwrap()
        }
        async fn rotate(&self, token: &str) -> Result<Rotation, AuthError> {
            rotate(&self.database, &self.jwt, token, &self.clock).await
        }
        async fn counts(&self) -> (i64, i64, i64) {
            sqlx::query_as("SELECT (SELECT count(*) FROM refresh_sessions), (SELECT count(*) FROM refresh_tokens), (SELECT count(*) FROM refresh_tokens WHERE consumed = 1)")
                .fetch_one(&self.database.db_pool).await.unwrap()
        }
    }
    fn unauthorized(result: Result<Rotation, AuthError>) {
        assert!(matches!(result, Err(AuthError::Unauthorized)));
    }
    fn internal<T>(result: Result<T, AuthError>) {
        assert!(matches!(result, Err(AuthError::Internal)));
    }

    #[tokio::test]
    async fn logout_revokes_current_or_consumed_hash_only_and_is_idempotent() {
        let f = Fixture::new().await;
        for consumed in [false, true] {
            let first = f.issue().await;
            let independent = f.issue().await;
            let current = f.rotate(&first.refresh_token).await.unwrap();
            let token = if consumed {
                &first.refresh_token
            } else {
                &current.credential.refresh_token
            };
            for _ in 0..2 {
                revoke(&f.database, token).await.unwrap();
            }
            unauthorized(f.rotate(&first.refresh_token).await);
            unauthorized(f.rotate(&current.credential.refresh_token).await);
            assert!(f.jwt.verify(&current.access.access_token).is_ok());
            assert!(f.rotate(&independent.refresh_token).await.is_ok());
        }
        for token in ["", "unknown"] {
            revoke(&f.database, token).await.unwrap();
        }
        let expired = f.issue().await;
        f.time.store(expired.expires_at, Ordering::SeqCst);
        revoke(&f.database, &expired.refresh_token).await.unwrap();
        unauthorized(f.rotate(&expired.refresh_token).await);
        f.database
            .delete_expired_refresh_sessions(expired.expires_at)
            .await
            .unwrap();
        revoke(&f.database, &expired.refresh_token).await.unwrap();
        assert_eq!(
            f.database
                .get_user_by_google_sub("google-subject")
                .await
                .unwrap()
                .id,
            f.user
        );
    }

    #[tokio::test]
    async fn logout_storage_failure_preserves_the_live_session() {
        let f = Fixture::new().await;
        let initial = f.issue().await;
        sqlx::query("CREATE TRIGGER fail_logout BEFORE UPDATE OF revoked ON refresh_sessions BEGIN SELECT RAISE(ABORT, 'test failure'); END")
            .execute(&f.database.db_pool).await.unwrap();
        internal(revoke(&f.database, &initial.refresh_token).await);
        let current = f.rotate(&initial.refresh_token).await.unwrap();
        internal(revoke(&f.database, &initial.refresh_token).await);
        assert!(f.jwt.verify(&current.access.access_token).is_ok());
        sqlx::query("DROP TRIGGER fail_logout")
            .execute(&f.database.db_pool)
            .await
            .unwrap();
        revoke(&f.database, &initial.refresh_token).await.unwrap();
        unauthorized(f.rotate(&current.credential.refresh_token).await);
        f.database.db_pool.close().await;
        internal(revoke(&f.database, "unknown").await);
    }

    #[tokio::test]
    async fn issuance_stores_only_hashes_and_preserves_identity_and_existing_data() {
        let f = Fixture::new().await;
        let now = chrono::Utc::now();
        f.database
            .create_lobby(crate::models::lobby::Lobby {
                id: crate::models::lobby::LobbyId::new([1, 2, 3]),
                created_at: now,
                expires_at: now + chrono::Duration::minutes(1),
            })
            .await
            .unwrap();
        let first = f.issue().await;
        let second = f.issue().await;
        assert_ne!(first.refresh_token, second.refresh_token);
        assert_eq!(
            base16ct::lower::decode_vec(&first.refresh_token)
                .unwrap()
                .len(),
            32
        );
        assert_eq!(first.expires_at, 1000 + SESSION_SECONDS);
        assert_eq!(second.expires_at, first.expires_at);
        let hashes: Vec<Vec<u8>> = sqlx::query_scalar("SELECT token_hash FROM refresh_tokens")
            .fetch_all(&f.database.db_pool)
            .await
            .unwrap();
        assert!(hashes.contains(&hash(&first.refresh_token)));
        assert!(
            hashes
                .iter()
                .all(|h| h.len() == 32 && h != first.refresh_token.as_bytes())
        );
        f.database.create_tables().await.unwrap();
        assert_eq!(f.counts().await, (2, 2, 0));
        assert_eq!(
            f.database
                .get_user_by_google_sub("google-subject")
                .await
                .unwrap()
                .id,
            f.user
        );
        let lobbies: i64 = sqlx::query_scalar("SELECT count(*) FROM lobbies")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
        assert_eq!(lobbies, 1);
    }

    #[tokio::test]
    async fn rotation_rejects_invalid_credentials_and_keeps_absolute_expiry() {
        let f = Fixture::new().await;
        let initial = f.issue().await;
        let access = f.jwt.issue(f.user).unwrap();
        for token in ["", "unknown", access.as_str()] {
            unauthorized(f.rotate(token).await);
        }
        f.time.store(4600, Ordering::SeqCst);
        assert!(f.jwt.verify(&access).is_err());
        let next = f.rotate(&initial.refresh_token).await.unwrap();
        assert_eq!(f.jwt.verify(&next.access.access_token).unwrap().id, f.user);
        assert_ne!(initial.refresh_token, next.credential.refresh_token);
        assert_eq!(f.counts().await, (1, 2, 1));
        f.time.store(1000 + SESSION_SECONDS - 1, Ordering::SeqCst);
        let last = f.rotate(&next.credential.refresh_token).await.unwrap();
        let expiry: i64 = sqlx::query_scalar("SELECT expires_at FROM refresh_sessions")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
        assert_eq!(expiry, 1000 + SESSION_SECONDS);
        assert_eq!(next.credential.expires_at, expiry);
        assert_eq!(last.credential.expires_at, expiry);
        assert_eq!(next.access.token_type, "Bearer");
        assert_eq!(next.access.expires_in, 3600);
        let json = serde_json::to_value(&next.access).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert!(json.get("refresh_token").is_none());
        f.time.store(expiry, Ordering::SeqCst);
        unauthorized(f.rotate(&last.credential.refresh_token).await);
        assert_eq!(f.counts().await, (1, 3, 2));
    }

    #[tokio::test]
    async fn consumed_token_reuse_revokes_only_its_session() {
        let f = Fixture::new().await;
        let first = f.issue().await;
        let independent = f.issue().await;
        let second = f.rotate(&first.refresh_token).await.unwrap();
        let third = f.rotate(&second.credential.refresh_token).await.unwrap();
        unauthorized(f.rotate(&first.refresh_token).await);
        unauthorized(f.rotate(&second.credential.refresh_token).await);
        unauthorized(f.rotate(&third.credential.refresh_token).await);
        assert!(f.jwt.verify(&third.access.access_token).is_ok());
        assert!(f.rotate(&independent.refresh_token).await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_consumers_cannot_both_rotate() {
        let f = Fixture::new().await;
        let initial = f.issue().await;
        let other = Fixture::connect(&f.file).await;
        let (a, b) = tokio::join!(
            f.rotate(&initial.refresh_token),
            rotate(&other, &f.jwt, &initial.refresh_token, &f.clock)
        );
        let success = match (a, b) {
            (Ok(token), Err(AuthError::Unauthorized))
            | (Err(AuthError::Unauthorized), Ok(token)) => token,
            _ => panic!("expected one rotation and one reuse rejection"),
        };
        unauthorized(f.rotate(&success.credential.refresh_token).await);
        assert_eq!(f.counts().await, (1, 2, 1));
        other.db_pool.close().await;
    }

    #[tokio::test]
    async fn persistence_and_issuance_failures_roll_back_without_credentials() {
        let f = Fixture::new().await;
        let initial = f.issue().await;
        sqlx::query("CREATE TRIGGER fail_token BEFORE INSERT ON refresh_tokens BEGIN SELECT RAISE(ABORT, 'test failure'); END")
            .execute(&f.database.db_pool).await.unwrap();
        internal(issue(&f.database, f.user, &f.clock).await);
        internal(f.rotate(&initial.refresh_token).await);
        assert_eq!(f.counts().await, (1, 1, 0));
        sqlx::query("DROP TRIGGER fail_token")
            .execute(&f.database.db_pool)
            .await
            .unwrap();
        // Force JWT issuance to fail after consumption but before commit.
        let bad_jwt = JwtAuth::new(b"test-secret", Arc::new(|| i64::MAX));
        internal(rotate(&f.database, &bad_jwt, &initial.refresh_token, &f.clock).await);
        internal(issue(&f.database, f.user, &(Arc::new(|| i64::MAX) as Clock)).await);
        assert_eq!(f.counts().await, (1, 1, 0));
        let next = f.rotate(&initial.refresh_token).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_revoke BEFORE UPDATE OF revoked ON refresh_sessions BEGIN SELECT RAISE(ABORT, 'test failure'); END")
            .execute(&f.database.db_pool).await.unwrap();
        internal(f.rotate(&initial.refresh_token).await);
        let revoked: bool = sqlx::query_scalar("SELECT revoked FROM refresh_sessions")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
        assert!(!revoked);
        sqlx::query("DROP TRIGGER fail_revoke")
            .execute(&f.database.db_pool)
            .await
            .unwrap();
        assert!(f.rotate(&next.credential.refresh_token).await.is_ok());
        assert_eq!(
            f.database
                .get_user_by_google_sub("google-subject")
                .await
                .unwrap()
                .id,
            f.user
        );
    }

    #[tokio::test]
    async fn sessions_and_consumed_history_survive_reconnection() {
        let mut f = Fixture::new().await;
        let first = f.issue().await;
        let second = f.rotate(&first.refresh_token).await.unwrap();
        f.database.db_pool.close().await;
        f.database = Fixture::connect(&f.file).await;
        f.database.create_tables().await.unwrap();
        let third = f.rotate(&second.credential.refresh_token).await.unwrap();
        assert_eq!(f.jwt.verify(&third.access.access_token).unwrap().id, f.user);
        unauthorized(f.rotate(&first.refresh_token).await);
        f.database.db_pool.close().await;
        f.database = Fixture::connect(&f.file).await;
        unauthorized(f.rotate(&third.credential.refresh_token).await);
    }
    #[tokio::test]
    async fn cleanup_retains_history_until_expiry_and_rolls_back_on_failure() {
        let f = Fixture::new().await;
        let first = f.issue().await;
        let next = f.rotate(&first.refresh_token).await.unwrap();
        let expiry = 1000 + SESSION_SECONDS;
        assert_eq!(
            f.database
                .delete_expired_refresh_sessions(expiry - 1)
                .await
                .unwrap(),
            0
        );
        assert_eq!(f.counts().await, (1, 2, 1));
        // Retained consumed tokens still trigger session-wide revocation.
        unauthorized(f.rotate(&first.refresh_token).await);
        unauthorized(f.rotate(&next.credential.refresh_token).await);
        assert_eq!(
            f.database
                .delete_expired_refresh_sessions(expiry - 1)
                .await
                .unwrap(),
            0
        );
        f.time.store(1001, Ordering::SeqCst);
        let active = f.issue().await;
        let active_next = f.rotate(&active.refresh_token).await.unwrap();
        assert_eq!(f.counts().await, (2, 4, 2));

        sqlx::query("CREATE TRIGGER fail_cleanup BEFORE DELETE ON refresh_sessions BEGIN SELECT RAISE(ABORT, 'test failure'); END")
            .execute(&f.database.db_pool).await.unwrap();
        assert!(
            f.database
                .delete_expired_refresh_sessions(expiry)
                .await
                .is_err()
        );
        assert_eq!(
            f.counts().await,
            (2, 4, 2),
            "token deletion must roll back with session deletion"
        );
        sqlx::query("DROP TRIGGER fail_cleanup")
            .execute(&f.database.db_pool)
            .await
            .unwrap();
        assert_eq!(
            f.database
                .delete_expired_refresh_sessions(expiry)
                .await
                .unwrap(),
            1
        );
        assert_eq!(f.counts().await, (1, 2, 1));
        assert_eq!(
            f.database
                .delete_expired_refresh_sessions(expiry)
                .await
                .unwrap(),
            0
        );
        assert!(
            f.rotate(&active_next.credential.refresh_token)
                .await
                .is_ok()
        );
        unauthorized(f.rotate(&active.refresh_token).await);
        assert_eq!(
            f.database
                .delete_expired_refresh_sessions(expiry + 1)
                .await
                .unwrap(),
            1
        );
        assert_eq!(f.counts().await, (0, 0, 0));
        assert_eq!(
            f.database
                .get_user_by_google_sub("google-subject")
                .await
                .unwrap()
                .id,
            f.user
        );
    }
}
