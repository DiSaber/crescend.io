mod api_error;
mod app_state;
mod auth;
mod database;
mod lobbies;
mod models;
mod queue;
mod routes;
mod search;
mod youtube;

use crate::{app_state::AppState, database::Database};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let auth = std::sync::Arc::new(
        auth::Auth::new(
            auth::AuthConfig::from_env().expect("Authentication configuration must be valid"),
        )
        .expect("Authentication initialization failed"),
    );
    let database = Database::connect()
        .await
        .expect("Database should be connected");
    database
        .create_tables()
        .await
        .expect("Database tables should be created");
    let search_config =
        search::config::SearchConfig::from_env().expect("Search configuration must be valid");
    if database.cleanup_catalog((auth.clock)()).await.is_err() {
        eprintln!("Catalog startup maintenance temporarily unavailable");
    }
    database
        .import_starters(
            include_str!("../resources/starter-songs.json"),
            (auth.clock)(),
        )
        .await
        .expect("Starter catalog import failed");
    database.start_cleanup();

    let app = routes::router(auth.jwt.clone()).with_state(AppState {
        search: search::SearchService::new(
            search_config,
            auth.clock.clone(),
            youtube::search::SearchClient::new().expect("Search client initialization failed"),
        ),
        metadata: youtube::MetadataClient::new().expect("Metadata client initialization failed"),
        database,
        auth,
        lobby_updates: Default::default(),
    });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.unwrap();
}
