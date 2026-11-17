//! Disk-backed fixtures for game acceptance tests. Time comes from an atomic the
//! tests move by hand, so phase timers never sleep.
use crate::{
    auth::AuthenticatedUser,
    database::Database,
    models::{game::*, youtube::VideoMetadata},
    youtube::MetadataClient,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use std::time::Duration;

mod g1_engine;
mod g1_http;
mod g2_modes;
mod g3_tune;
mod g4_prompts;
mod g5_saved;
mod g6_lifecycle;
mod g6_load;
mod g6_nicknames;
mod ghosts;

const NOW: i64 = 1_800_000_000;

struct Fixture {
    database: Database,
    _directory: tempfile::TempDir,
    metadata: MetadataClient,
    upstream: wiremock::MockServer,
    time: Arc<AtomicI64>,
    auth: Arc<crate::auth::Auth>,
    updates: crate::lobbies::updates::LobbyUpdates,
}

struct Lobby {
    host: AuthenticatedUser,
    host_membership: String,
    guests: Vec<(AuthenticatedUser, String)>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(directory.path().join("game.db"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            // Same waits as production (`Database::connect`).
            .busy_timeout(Duration::from_secs(5));
        let database = Database {
            db_pool: SqlitePoolOptions::new()
                .max_connections(8)
                .acquire_timeout(Duration::from_secs(5))
                .connect_with(options)
                .await
                .unwrap(),
        };
        database.create_tables().await.unwrap();
        let upstream = wiremock::MockServer::start().await;
        let metadata = MetadataClient::for_test(upstream.uri().parse().unwrap());
        let time = Arc::new(AtomicI64::new(NOW));
        let clock = time.clone();
        let config = crate::auth::AuthConfig::load(|key| {
            Some(
                match key {
                    "GOOGLE_CLIENT_ID" => "test-client",
                    "GOOGLE_CLIENT_SECRET" => "test-secret",
                    "GOOGLE_REDIRECT_URI" => "http://localhost:3000/api/auth/google/callback",
                    "JWT_SIGNING_SECRET" => "game-test-signing-secret-at-least-32-bytes",
                    _ => return None,
                }
                .into(),
            )
        })
        .unwrap();
        let auth = Arc::new(
            crate::auth::Auth::with_clock(config, Arc::new(move || clock.load(Ordering::SeqCst)))
                .unwrap(),
        );
        Self {
            database,
            _directory: directory,
            metadata,
            upstream,
            time,
            auth,
            updates: Default::default(),
        }
    }

    fn now(&self) -> i64 {
        self.time.load(Ordering::SeqCst)
    }

    fn advance(&self, seconds: i64) {
        self.time.fetch_add(seconds, Ordering::SeqCst);
    }

    async fn account(&self, name: &str) -> AuthenticatedUser {
        static NEXT: AtomicI64 = AtomicI64::new(0);
        let name = format!("{name}-{}", NEXT.fetch_add(1, Ordering::SeqCst));
        AuthenticatedUser {
            id: self.database.create_user(&name).await.unwrap().id,
            expires_at: NOW + 86_400,
        }
    }

    /// A lobby with a host and `guests` guests.
    async fn lobby(&self, guests: usize) -> Lobby {
        let host = self.account("host").await;
        let view = self
            .database
            .create_owned_lobby(host.id, || self.now())
            .await
            .unwrap();
        let code = view.lobby.unwrap().join_code;
        let mut joined = Vec::new();
        for i in 0..guests {
            let user = self.account(&format!("guest{i}")).await;
            let view = self
                .database
                .join_lobby(user.id, &code, || self.now())
                .await
                .unwrap();
            joined.push((user, view.membership_id.unwrap().as_str().to_owned()));
        }
        Lobby {
            host,
            host_membership: view.membership_id.unwrap().as_str().into(),
            guests: joined,
        }
    }

    async fn start(&self, lobby: &Lobby, mode: &str) -> GameSnapshot {
        self.start_with(lobby, mode, serde_json::json!({}))
            .await
            .unwrap()
    }

    async fn start_with(
        &self,
        lobby: &Lobby,
        mode: &str,
        settings: serde_json::Value,
    ) -> Result<GameSnapshot, crate::game::GameError> {
        let request: StartGameRequest =
            serde_json::from_value(serde_json::json!({"mode_id": mode, "settings": settings}))
                .unwrap();
        let (mode_id, definition) = crate::game::resolve(&request, None)?;
        self.database
            .start_game(
                &lobby.host,
                &lobby.host_membership,
                &mode_id,
                &definition,
                &[],
                || self.now(),
            )
            .await
    }

    async fn view(&self, user: &AuthenticatedUser, membership: &str) -> GameView {
        self.database
            .game_view(user, membership, || self.now())
            .await
            .unwrap()
            .game
            .unwrap()
    }

    async fn submit(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        video: &str,
    ) -> Result<GameSubmissionResponse, crate::game::GameError> {
        self.database
            .submit_game(
                user,
                membership,
                &request_id(user, video),
                &song(video),
                || self.now(),
            )
            .await
    }

    async fn vote(
        &self,
        user: &AuthenticatedUser,
        membership: &str,
        submission: &str,
    ) -> Result<GameMutationResponse, crate::game::GameError> {
        self.database
            .vote_game(user, membership, submission, || self.now())
            .await
    }

    async fn tick(&self) -> Vec<(crate::models::lobby::LobbyId, i64)> {
        self.database.tick_games(self.now()).await.unwrap()
    }

    async fn revision(&self, lobby: &Lobby) -> i64 {
        sqlx::query_scalar(
            "SELECT l.revision FROM lobbies l JOIN lobby_memberships m ON m.lobby_id = l.id WHERE m.id = ?",
        )
        .bind(&lobby.host_membership)
        .fetch_one(&self.database.db_pool)
        .await
        .unwrap()
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

    fn request(
        &self,
        method: reqwest::Method,
        url: String,
        user: &AuthenticatedUser,
    ) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method, url)
            .bearer_auth(self.auth.jwt.issue(user.id).unwrap())
    }

    async fn mock_metadata(&self, title: &str, author: &str) {
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "title": title, "author_name": author,
                "author_url": "https://youtube.com/@server", "thumbnail_url": "https://i.ytimg.com/image.jpg"
            })))
            .mount(&self.upstream)
            .await;
    }
}

fn request_id(user: &AuthenticatedUser, video: &str) -> String {
    let mut id = format!(
        "{:x}{}",
        user.id,
        base16ct::lower::encode_string(video.as_bytes())
    );
    id.truncate(32);
    format!("{id:0>32}")
}

fn song(video: &str) -> VideoMetadata {
    VideoMetadata {
        video_id: video.into(),
        title: format!("Artist - Song {video} (Official Video)"),
        author_name: "Artist".into(),
        author_url: "https://youtube.com/@artist".into(),
        thumbnail_url: "https://i.ytimg.com/image.jpg".into(),
        embed_url: format!("https://www.youtube.com/embed/{video}?autoplay=1&enablejsapi=1"),
    }
}
