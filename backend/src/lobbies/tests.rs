use crate::{
    app_state::AppState,
    auth::{Auth, AuthConfig},
    database::Database,
};
use reqwest::{Client, Response};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

mod us1_http;
mod us1_storage;
mod us2_http;
mod us2_storage;
mod us3_current;

const NOW: i64 = 1_800_000_000;

struct Fixture {
    database: Database,
    auth: Arc<Auth>,
    time: Arc<AtomicI64>,
    user: i64,
    client: Client,
    base: String,
    server: tokio::task::JoinHandle<()>,
    directory: tempfile::TempDir,
}

impl Fixture {
    async fn connect(path: &Path) -> Database {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(1));
        Database {
            db_pool: SqlitePoolOptions::new()
                .max_connections(4)
                .acquire_timeout(Duration::from_secs(2))
                .connect_with(options)
                .await
                .unwrap(),
        }
    }

    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = Self::connect(&directory.path().join("test.db")).await;
        database.create_tables().await.unwrap();
        let user = database
            .create_user("private-provider-subject")
            .await
            .unwrap()
            .id;
        let time = Arc::new(AtomicI64::new(NOW));
        let clock = time.clone();
        let config = AuthConfig::load(|key| {
            Some(
                match key {
                    "GOOGLE_CLIENT_ID" => "test-client",
                    "GOOGLE_CLIENT_SECRET" => "test-secret",
                    "GOOGLE_REDIRECT_URI" => "http://localhost:3000/api/auth/google/callback",
                    "JWT_SIGNING_SECRET" => "test-only-signing-secret-at-least-32-bytes",
                    _ => return None,
                }
                .into(),
            )
        })
        .unwrap();
        let auth = Arc::new(
            Auth::with_clock(config, Arc::new(move || clock.load(Ordering::SeqCst))).unwrap(),
        );
        let app = crate::routes::router(auth.jwt.clone()).with_state(AppState {
            database: database.clone(),
            auth: auth.clone(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            database,
            auth,
            time,
            user,
            client: Client::new(),
            base,
            server,
            directory,
        }
    }

    async fn create(&self) -> Response {
        self.client
            .post(format!("{}/api/lobbies", self.base))
            .bearer_auth(self.auth.jwt.issue(self.user).unwrap())
            .send()
            .await
            .unwrap()
    }

    async fn count(&self, table: &str) -> i64 {
        let query = match table {
            "lobbies" => "SELECT count(*) FROM lobbies",
            "lobby_memberships" => "SELECT count(*) FROM lobby_memberships",
            _ => panic!("unknown test table"),
        };
        sqlx::query_scalar(query)
            .fetch_one(&self.database.db_pool)
            .await
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
