use std::{str::FromStr, time::Duration};

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};

use crate::models::lobby::{Lobby, LobbyId};

pub const LOBBY_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
pub const LOBBY_LIFETIME: TimeDelta = TimeDelta::minutes(1);

#[derive(Debug, Clone)]
pub struct Database {
    pub db_pool: SqlitePool,
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

        Ok(())
    }

    pub async fn create_lobby(
        &self,
        id: LobbyId,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let expires_at = created_at + LOBBY_LIFETIME;
        sqlx::query(
            r#"
            INSERT INTO lobbies (id, created_at, expires_at)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(id)
        .bind(created_at)
        .bind(expires_at)
        .execute(&self.db_pool)
        .await?;

        Ok(())
    }

    pub async fn get_active_lobby(
        &self,
        id: LobbyId,
        now: DateTime<Utc>,
    ) -> Result<Option<Lobby>, sqlx::Error> {
        sqlx::query_as(
            r#"
            SELECT id, created_at, expires_at
            FROM lobbies
            WHERE id = ? AND expires_at > ?
            "#,
        )
        .bind(id)
        .bind(now)
        .fetch_optional(&self.db_pool)
        .await
    }

    pub async fn delete_expired_lobbies(&self, now: DateTime<Utc>) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM lobbies WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.db_pool)
            .await?;

        Ok(result.rows_affected())
    }

    pub fn start_lobby_cleanup(&self) {
        let database = self.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(LOBBY_CLEANUP_INTERVAL);

            loop {
                interval.tick().await;

                if let Err(error) = database.delete_expired_lobbies(Utc::now()).await {
                    eprintln!("failed to delete expired lobbies: {error}");
                }
            }
        });
    }
}
