use crate::{
    app_state::AppState,
    auth::{Auth, AuthConfig, AuthenticatedUser},
    database::Database,
    models::search::Snapshot,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
const NOW: i64 = 1_800_000_000;
mod catalog;
mod http;

struct Fixture {
    database: Database,
    directory: tempfile::TempDir,
    time: Arc<AtomicI64>,
    auth: Arc<Auth>,
    upstream: wiremock::MockServer,
}
struct Server {
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn connect(path: &std::path::Path) -> Database {
        Database {
            db_pool: SqlitePoolOptions::new()
                .max_connections(8)
                .connect_with(
                    SqliteConnectOptions::new()
                        .filename(path)
                        .create_if_missing(true)
                        .foreign_keys(true)
                        .journal_mode(SqliteJournalMode::Wal)
                        .busy_timeout(std::time::Duration::from_secs(1)),
                )
                .await
                .unwrap(),
        }
    }
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = Self::connect(&directory.path().join("search.db")).await;
        database.create_tables().await.unwrap();
        let time = Arc::new(AtomicI64::new(NOW));
        let clock = time.clone();
        let config = AuthConfig::load(|k| {
            Some(
                match k {
                    "GOOGLE_CLIENT_ID" => "test-client",
                    "GOOGLE_CLIENT_SECRET" => "test-secret",
                    "GOOGLE_REDIRECT_URI" => "http://localhost:3000/api/auth/google/callback",
                    "JWT_SIGNING_SECRET" => "search-test-signing-secret-at-least-32-bytes",
                    _ => return None,
                }
                .into(),
            )
        })
        .unwrap();
        let auth = Arc::new(
            Auth::with_clock(config, Arc::new(move || clock.load(Ordering::SeqCst))).unwrap(),
        );
        Self {
            database,
            directory,
            time,
            auth,
            upstream: wiremock::MockServer::start().await,
        }
    }
    fn now(&self) -> i64 {
        self.time.load(Ordering::SeqCst)
    }
    async fn account(&self, name: &str) -> AuthenticatedUser {
        AuthenticatedUser {
            id: self.database.create_user(name).await.unwrap().id,
            expires_at: NOW + 3600,
        }
    }
    fn state(&self) -> AppState {
        AppState {
            database: self.database.clone(),
            auth: self.auth.clone(),
            metadata: crate::youtube::MetadataClient::for_test(
                self.upstream.uri().parse().unwrap(),
            ),
            search: super::SearchService::disabled(),
            lobby_updates: Default::default(),
        }
    }
    async fn server(&self, state: AppState) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = crate::routes::router(state.auth.jwt.clone()).with_state(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Server { base, task }
    }
}

fn song(id: &str, title: &str, artist: &str) -> Snapshot {
    Snapshot {
        video_id: id.into(),
        title: title.into(),
        artist: artist.into(),
        thumbnail_url: None,
        watch_url: format!("https://www.youtube.com/watch?v={id}"),
    }
}

#[test]
fn normalization_and_configuration_boundaries() {
    use super::{config::SearchConfig, normalize::*};
    assert_eq!(query("  CÀFÉ  ΜΆΪΟΣ "), Some("cafe μαιοσ".into()));
    assert!(query(&"é".repeat(200)).is_some());
    assert!(query(&"é".repeat(201)).is_none());
    assert!(query(" \n ").is_none());
    assert_eq!(
        terms("OR title:foo -bar"),
        Some("\"OR\"* AND \"title\"* AND \"foo\"* AND \"bar\"*".into())
    );
    assert_eq!(terms("***"), None);
    assert_ne!(normalize("a-b"), normalize("a b"));
    assert_eq!(SearchConfig::load(|_| None).unwrap().daily_limit, 90);
    for bad in ["-1", "", "90.0", "9223372036854775808", "secret"] {
        assert!(
            SearchConfig::load(|k| (k == "YOUTUBE_SEARCH_DAILY_LIMIT").then(|| bad.into()))
                .is_err()
        );
    }
}
mod allowance;
mod discovery;
mod load;
mod queue_growth;
