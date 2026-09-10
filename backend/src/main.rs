mod app_state;
mod database;
mod models;
mod routes;

use crate::{app_state::AppState, database::Database};

#[tokio::main]
async fn main() {
    let database = Database::connect()
        .await
        .expect("Database should be connected");
    database
        .create_tables()
        .await
        .expect("Database tables should be created");
    database.start_lobby_cleanup();

    let app = routes::router().with_state(AppState { database });

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.unwrap();
}
