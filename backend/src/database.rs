use std::{str::FromStr, time::Duration};

use chrono::{DateTime, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};

use crate::models::lobby::Lobby;
use crate::models::user::User;

pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Database {
    pub db_pool: SqlitePool,
}

/// Refresh rejection is separate from storage/credential-generation failure.
#[derive(Debug)]
pub enum RefreshSessionError {
    InvalidCredential,
    Internal,
}

impl From<sqlx::Error> for RefreshSessionError {
    fn from(_: sqlx::Error) -> Self {
        Self::Internal
    }
}

impl Database {
    pub async fn connect() -> Result<Self, sqlx::Error> {
        let opts = SqliteConnectOptions::from_str("sqlite://data.db")
            .expect("Url should be valid")
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);
        let db_pool = SqlitePool::connect_with(opts).await?;

        Ok(Self { db_pool })
    }

    /// Creates the db tables when they don't exist.
    pub async fn create_tables(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS lobbies (
                id BLOB PRIMARY KEY NOT NULL CHECK (length(id) = 3),
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL CHECK (expires_at >= created_at)
            ) STRICT
            "#,
        )
        .execute(&self.db_pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY NOT NULL,
                google_sub TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL
            ) STRICT",
        )
        .execute(&self.db_pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS refresh_sessions (
                id INTEGER PRIMARY KEY NOT NULL,
                user_id INTEGER NOT NULL REFERENCES users(id),
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL CHECK (expires_at = created_at + 2592000),
                revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1))
            ) STRICT",
        )
        .execute(&self.db_pool)
        .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS refresh_tokens (
                token_hash BLOB PRIMARY KEY NOT NULL CHECK (length(token_hash) = 32),
                session_id INTEGER NOT NULL REFERENCES refresh_sessions(id),
                consumed INTEGER NOT NULL DEFAULT 0 CHECK (consumed IN (0, 1))
            ) STRICT",
        )
        .execute(&self.db_pool)
        .await?;

        Ok(())
    }

    /// Only call after verifying the Google ID token.
    pub async fn create_user(&self, google_sub: &str) -> Result<User, sqlx::Error> {
        sqlx::query_as(
            "INSERT INTO users (google_sub, created_at) VALUES (?, ?)
             RETURNING id, google_sub, created_at",
        )
        .bind(google_sub)
        .bind(Utc::now())
        .fetch_one(&self.db_pool)
        .await
    }

    /// Returns `RowNotFound` when no user has this Google subject.
    pub async fn get_user_by_google_sub(&self, google_sub: &str) -> Result<User, sqlx::Error> {
        sqlx::query_as("SELECT id, google_sub, created_at FROM users WHERE google_sub = ?")
            .bind(google_sub)
            .fetch_one(&self.db_pool)
            .await
    }

    /// Commit the session and initial token hash together before credentials are returned.
    pub async fn create_refresh_session(
        &self,
        user: i64,
        created_at: i64,
        expires_at: i64,
        token_hash: &[u8],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.db_pool.begin().await?;
        let session: i64 = sqlx::query_scalar(
            "INSERT INTO refresh_sessions (user_id, created_at, expires_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(user).bind(created_at).bind(expires_at).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO refresh_tokens (token_hash, session_id) VALUES (?, ?)")
            .bind(token_hash)
            .bind(session)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// Generate credentials inside the transaction so issuance failures roll back consumption.
    /// The callback supplies only the replacement hash for persistence; its response is returned
    /// after commit. Consumed-token reuse commits revocation before rejecting the request.
    pub async fn rotate_refresh_session<T>(
        &self,
        token_hash: &[u8],
        clock: impl FnOnce() -> i64,
        issue: impl FnOnce(i64, i64) -> Result<(T, Vec<u8>), RefreshSessionError>,
    ) -> Result<T, RefreshSessionError> {
        // Reserve the SQLite writer before reading, including across processes/connections.
        let mut tx = self.db_pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<(i64, i64, i64, bool, bool)> = sqlx::query_as(
            "SELECT s.id, s.user_id, s.expires_at, s.revoked, t.consumed
             FROM refresh_tokens t JOIN refresh_sessions s ON s.id = t.session_id WHERE t.token_hash = ?",
        ).bind(token_hash).fetch_optional(&mut *tx).await?;
        let (session, user, expires, revoked, consumed) =
            row.ok_or(RefreshSessionError::InvalidCredential)?;
        if revoked || expires <= clock() {
            return Err(RefreshSessionError::InvalidCredential);
        }
        if consumed {
            sqlx::query("UPDATE refresh_sessions SET revoked = 1 WHERE id = ?")
                .bind(session)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Err(RefreshSessionError::InvalidCredential);
        }
        sqlx::query("UPDATE refresh_tokens SET consumed = 1 WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&mut *tx)
            .await?;
        let (response, replacement_hash) = issue(user, expires)?;
        sqlx::query("INSERT INTO refresh_tokens (token_hash, session_id) VALUES (?, ?)")
            .bind(replacement_hash)
            .bind(session)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(response)
    }

    /// A single committed update revokes the session for either a current or consumed hash.
    /// Unknown credentials and repeated revocations are idempotent; other sessions are untouched.
    pub async fn revoke_refresh_session(&self, token_hash: &[u8]) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE refresh_sessions SET revoked = 1 WHERE revoked = 0 AND id IN
            (SELECT session_id FROM refresh_tokens WHERE token_hash = ?)",
        )
        .bind(token_hash)
        .execute(&self.db_pool)
        .await?;
        Ok(())
    }

    pub async fn create_lobby(&self, lobby: Lobby) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO lobbies (id, created_at, expires_at)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(lobby.id)
        .bind(lobby.created_at)
        .bind(lobby.expires_at)
        .execute(&self.db_pool)
        .await?;

        Ok(())
    }

    pub async fn delete_expired_lobbies(&self, now: DateTime<Utc>) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM lobbies WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.db_pool)
            .await?;

        Ok(result.rows_affected())
    }

    /// Retain all token history until the session's absolute expiry, then delete
    /// child tokens and their sessions atomically. Returns the number of sessions removed.
    pub async fn delete_expired_refresh_sessions(&self, now: i64) -> Result<u64, sqlx::Error> {
        let mut tx = self.db_pool.begin().await?;
        sqlx::query(
            "DELETE FROM refresh_tokens WHERE session_id IN
             (SELECT id FROM refresh_sessions WHERE expires_at <= ?)",
        )
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query("DELETE FROM refresh_sessions WHERE expires_at <= ?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    pub fn start_cleanup(&self) {
        let database = self.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CLEANUP_INTERVAL);

            loop {
                interval.tick().await;

                let now = Utc::now();
                if let Err(error) = database.delete_expired_lobbies(now).await {
                    eprintln!("failed to delete expired lobbies: {error}");
                }
                if let Err(error) = database
                    .delete_expired_refresh_sessions(now.timestamp())
                    .await
                {
                    eprintln!("failed to delete expired refresh sessions: {error}");
                }
            }
        });
    }
}
