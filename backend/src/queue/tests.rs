//! Disk-backed fixtures shared by queue acceptance tests.
use crate::{database::Database, youtube::MetadataClient};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::time::Duration;

mod lifecycle;
mod us1_http;
mod us1_submissions;
mod us2_events;
mod us2_load;
mod us2_synchronization;
mod us3_http;
mod us3_removal;
mod us4_http;
mod us4_progression;
mod us5_http;
mod us5_membership;
mod us5_votes;
mod us6_http;
mod us6_reordering;

struct HttpFixture {
    base: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl HttpFixture {
    fn queue(&self, membership: &str) -> String {
        format!("{}/api/lobbies/memberships/{membership}/queue", self.base)
    }
}

struct Fixture {
    database: Database,
    directory: tempfile::TempDir,
    metadata: MetadataClient,
    upstream: wiremock::MockServer,
    time: std::sync::Arc<std::sync::atomic::AtomicI64>,
    auth: std::sync::Arc<crate::auth::Auth>,
    updates: crate::lobbies::updates::LobbyUpdates,
}

const NOW: i64 = 1_800_000_000;

#[tokio::test]
async fn fixture_schema_and_authorization() {
    use crate::database::queue::{Permission, authorize};
    let f = Fixture::new().await;
    let (host, membership, code) = f.host().await;
    let (guest, guest_membership) = f.guest("guest", &code).await;
    let mut tx = f.database.db_pool.begin().await.unwrap();
    let bound = authorize(&mut tx, &host, &membership, Permission::Host, &|| f.now())
        .await
        .unwrap();
    assert_eq!(bound.revision.value(), 2);
    authorize(
        &mut tx,
        &guest,
        &guest_membership,
        Permission::Guest,
        &|| f.now(),
    )
    .await
    .unwrap();
    assert!(matches!(
        authorize(&mut tx, &guest, &membership, Permission::Member, &|| f
            .now())
        .await,
        Err(crate::queue::QueueError::MembershipForbidden)
    ));
    tx.commit().await.unwrap();
    f.database.create_tables().await.unwrap();
    let foreign_keys: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&f.database.db_pool)
        .await
        .unwrap();
    assert!(foreign_keys.is_empty());
    let (base, server) = f.http().await;
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{base}/api/lobbies/current"))
            .bearer_auth(f.auth.jwt.issue(host.id).unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    server.abort();
}

impl Fixture {
    async fn connect(path: &std::path::Path) -> Database {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(1));
        Database {
            db_pool: SqlitePoolOptions::new()
                .max_connections(8)
                .acquire_timeout(Duration::from_secs(2))
                .connect_with(options)
                .await
                .unwrap(),
        }
    }

    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database = Self::connect(&directory.path().join("queue.db")).await;
        database.create_tables().await.unwrap();
        let upstream = wiremock::MockServer::start().await;
        let metadata = MetadataClient::for_test(upstream.uri().parse().unwrap());
        let time = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(NOW));
        let clock = time.clone();
        let config = crate::auth::AuthConfig::load(|key| {
            Some(
                match key {
                    "GOOGLE_CLIENT_ID" => "test-client",
                    "GOOGLE_CLIENT_SECRET" => "test-secret",
                    "GOOGLE_REDIRECT_URI" => "http://localhost:3000/api/auth/google/callback",
                    "JWT_SIGNING_SECRET" => "queue-test-signing-secret-at-least-32-bytes",
                    _ => return None,
                }
                .into(),
            )
        })
        .unwrap();
        let auth = std::sync::Arc::new(
            crate::auth::Auth::with_clock(
                config,
                std::sync::Arc::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)),
            )
            .unwrap(),
        );
        Self {
            database,
            directory,
            metadata,
            upstream,
            time,
            auth,
            updates: Default::default(),
        }
    }

    fn now(&self) -> i64 {
        self.time.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn account(&self, name: &str) -> crate::auth::AuthenticatedUser {
        crate::auth::AuthenticatedUser {
            id: self.database.create_user(name).await.unwrap().id,
            expires_at: NOW + 3600,
        }
    }

    async fn host(&self) -> (crate::auth::AuthenticatedUser, String, String) {
        let user = self.account("host").await;
        let view = self
            .database
            .create_owned_lobby(user.id, || self.now())
            .await
            .unwrap();
        (
            user,
            view.membership_id.unwrap().as_str().into(),
            view.lobby.unwrap().join_code,
        )
    }

    async fn guest(&self, name: &str, code: &str) -> (crate::auth::AuthenticatedUser, String) {
        let user = self.account(name).await;
        let view = self
            .database
            .join_lobby(user.id, code, || self.now())
            .await
            .unwrap();
        (user, view.membership_id.unwrap().as_str().into())
    }

    async fn fail_revision(&self) {
        sqlx::query("CREATE TRIGGER fail_queue_revision BEFORE UPDATE OF revision ON lobbies BEGIN SELECT RAISE(ABORT, 'injected rollback'); END")
            .execute(&self.database.db_pool).await.unwrap();
    }

    async fn http(&self) -> (String, tokio::task::JoinHandle<()>) {
        let app =
            crate::routes::router(self.auth.jwt.clone()).with_state(crate::app_state::AppState {
                search: crate::search::SearchService::disabled(),
                metadata: self.metadata.clone(),
                database: self.database.clone(),
                auth: self.auth.clone(),
                lobby_updates: self.updates.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (base, task)
    }

    async fn server(&self) -> HttpFixture {
        let (base, task) = self.http().await;
        HttpFixture { base, task }
    }

    fn request(
        &self,
        method: reqwest::Method,
        url: String,
        user: &crate::auth::AuthenticatedUser,
    ) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method, url)
            .bearer_auth(self.auth.jwt.issue(user.id).unwrap())
    }

    async fn mock_metadata(&self) {
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "title":"Server title", "author_name":"Server channel", "author_url":"https://youtube.com/@server", "thumbnail_url":"https://i.ytimg.com/image.jpg"
            }))).mount(&self.upstream).await;
    }

    async fn seed(
        &self,
        user: &crate::auth::AuthenticatedUser,
        membership: &str,
        count: u32,
    ) -> Vec<crate::models::queue::EntryId> {
        let video = crate::models::youtube::VideoMetadata {
            video_id: "dQw4w9WgXcQ".into(),
            title: "Seed title".into(),
            author_name: "Channel".into(),
            author_url: "https://youtube.com/@channel".into(),
            thumbnail_url: "https://i.ytimg.com/image.jpg".into(),
            embed_url: "https://youtube.com/embed/dQw4w9WgXcQ".into(),
        };
        let mut ids = Vec::new();
        for i in 0..count {
            ids.push(
                self.database
                    .submit_queue(
                        user,
                        membership,
                        &format!("{i:032x}"),
                        &video,
                        || self.now(),
                        || {
                            crate::models::queue::EntryId::random()
                                .map_err(|_| crate::queue::QueueError::Unavailable)
                        },
                    )
                    .await
                    .unwrap()
                    .entry_id,
            );
        }
        ids
    }
}
