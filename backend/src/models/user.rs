use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub google_sub: String,
    pub created_at: DateTime<Utc>,
}
