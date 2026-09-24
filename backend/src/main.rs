mod app_state;
mod auth;
mod database;
mod models;
mod routes;

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
    database.start_cleanup();

    let app = routes::router(auth.jwt.clone()).with_state(AppState { database, auth });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.unwrap();
}
