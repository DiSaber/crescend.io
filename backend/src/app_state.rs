use std::sync::Arc;

use crate::{auth::Auth, database::Database};

#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub lobby_updates: crate::lobbies::updates::LobbyUpdates,
    pub auth: Arc<Auth>,
}
