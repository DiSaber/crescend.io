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
    pub refresh_token: String,
}

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn credentials(jwt: &JwtAuth, user: i64) -> Result<TokenResponse, AuthError> {
    let mut random = [0u8; 32];
    rand::rngs::SysRng
        .try_fill_bytes(&mut random)
        .map_err(|_| AuthError::Internal)?;
    let refresh_token = base16ct::lower::encode_string(&random);
    Ok(TokenResponse {
        access_token: jwt.issue(user)?,
        token_type: "Bearer".into(),
        expires_in: TOKEN_SECONDS,
        refresh_token,
    })
}

pub async fn issue(
    database: &Database,
    jwt: &JwtAuth,
    user: i64,
    clock: &Clock,
) -> Result<TokenResponse, AuthError> {
    let now = clock();
    let expires = now
        .checked_add(SESSION_SECONDS)
        .ok_or(AuthError::Internal)?;
    let response = credentials(jwt, user)?;
    database
        .create_refresh_session(user, now, expires, &hash(&response.refresh_token))
        .await
        .map_err(|_| AuthError::Internal)?;
    Ok(response)
}

pub async fn rotate(
    database: &Database,
    jwt: &JwtAuth,
    token: &str,
    clock: &Clock,
) -> Result<TokenResponse, AuthError> {
    if token.is_empty() {
        return Err(AuthError::Unauthorized);
    }
    database
        .rotate_refresh_session(
            &hash(token),
            || clock(),
            |user| {
                let response = credentials(jwt, user).map_err(|_| RefreshSessionError::Internal)?;
                let replacement_hash = hash(&response.refresh_token);
                Ok((response, replacement_hash))
            },
        )
        .await
        .map_err(|error| match error {
            RefreshSessionError::InvalidCredential => AuthError::Unauthorized,
            RefreshSessionError::Internal => AuthError::Internal,
        })
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
        async fn issue(&self) -> TokenResponse {
            issue(&self.database, &self.jwt, self.user, &self.clock)
                .await
                .unwrap()
        }
        async fn rotate(&self, token: &str) -> Result<TokenResponse, AuthError> {
            rotate(&self.database, &self.jwt, token, &self.clock).await
        }
        async fn counts(&self) -> (i64, i64, i64) {
            sqlx::query_as("SELECT (SELECT count(*) FROM refresh_sessions), (SELECT count(*) FROM refresh_tokens), (SELECT count(*) FROM refresh_tokens WHERE consumed = 1)")
                .fetch_one(&self.database.db_pool).await.unwrap()
        }
    }
    fn unauthorized(result: Result<TokenResponse, AuthError>) {
        assert!(matches!(result, Err(AuthError::Unauthorized)));
    }
    fn internal(result: Result<TokenResponse, AuthError>) {
        assert!(matches!(result, Err(AuthError::Internal)));
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
        assert_eq!(first.token_type, "Bearer");
        assert_eq!(first.expires_in, 3600);
        assert_eq!(f.jwt.verify(&first.access_token).unwrap().id, f.user);
        assert_eq!(f.jwt.verify(&second.access_token).unwrap().id, f.user);
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
        for token in ["", "unknown", initial.access_token.as_str()] {
            unauthorized(f.rotate(token).await);
        }
        f.time.store(4600, Ordering::SeqCst);
        assert!(f.jwt.verify(&initial.access_token).is_err());
        let next = f.rotate(&initial.refresh_token).await.unwrap();
        assert_eq!(f.jwt.verify(&next.access_token).unwrap().id, f.user);
        assert_ne!(initial.refresh_token, next.refresh_token);
        assert_eq!(f.counts().await, (1, 2, 1));
        f.time.store(1000 + SESSION_SECONDS - 1, Ordering::SeqCst);
        let last = f.rotate(&next.refresh_token).await.unwrap();
        let expiry: i64 = sqlx::query_scalar("SELECT expires_at FROM refresh_sessions")
            .fetch_one(&f.database.db_pool)
            .await
            .unwrap();
        assert_eq!(expiry, 1000 + SESSION_SECONDS);
        f.time.store(expiry, Ordering::SeqCst);
        unauthorized(f.rotate(&last.refresh_token).await);
        assert_eq!(f.counts().await, (1, 3, 2));
    }

    #[tokio::test]
    async fn consumed_token_reuse_revokes_only_its_session() {
        let f = Fixture::new().await;
        let first = f.issue().await;
        let independent = f.issue().await;
        let second = f.rotate(&first.refresh_token).await.unwrap();
        let third = f.rotate(&second.refresh_token).await.unwrap();
        unauthorized(f.rotate(&first.refresh_token).await);
        unauthorized(f.rotate(&second.refresh_token).await);
        unauthorized(f.rotate(&third.refresh_token).await);
        assert!(f.jwt.verify(&third.access_token).is_ok());
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
        unauthorized(f.rotate(&success.refresh_token).await);
        assert_eq!(f.counts().await, (1, 2, 1));
        other.db_pool.close().await;
    }

    #[tokio::test]
    async fn persistence_and_issuance_failures_roll_back_without_credentials() {
        let f = Fixture::new().await;
        let initial = f.issue().await;
        sqlx::query("CREATE TRIGGER fail_token BEFORE INSERT ON refresh_tokens BEGIN SELECT RAISE(ABORT, 'test failure'); END")
            .execute(&f.database.db_pool).await.unwrap();
        internal(issue(&f.database, &f.jwt, f.user, &f.clock).await);
        internal(f.rotate(&initial.refresh_token).await);
        assert_eq!(f.counts().await, (1, 1, 0));
        sqlx::query("DROP TRIGGER fail_token")
            .execute(&f.database.db_pool)
            .await
            .unwrap();
        // Force JWT issuance to fail after consumption but before commit.
        let bad_jwt = JwtAuth::new(b"test-secret", Arc::new(|| i64::MAX));
        internal(rotate(&f.database, &bad_jwt, &initial.refresh_token, &f.clock).await);
        internal(issue(&f.database, &bad_jwt, f.user, &f.clock).await);
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
        assert!(f.rotate(&next.refresh_token).await.is_ok());
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
        let third = f.rotate(&second.refresh_token).await.unwrap();
        assert_eq!(f.jwt.verify(&third.access_token).unwrap().id, f.user);
        unauthorized(f.rotate(&first.refresh_token).await);
        f.database.db_pool.close().await;
        f.database = Fixture::connect(&f.file).await;
        unauthorized(f.rotate(&third.refresh_token).await);
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
        unauthorized(f.rotate(&next.refresh_token).await);
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
        assert!(f.rotate(&active_next.refresh_token).await.is_ok());
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
