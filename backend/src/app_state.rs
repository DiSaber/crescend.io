use std::sync::Arc;

use crate::{auth::Auth, database::Database};

#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub auth: Arc<Auth>,
}
