use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub google_sub: String,
    pub created_at: DateTime<Utc>,
}

/// Guest accounts share the `users` table with Google accounts. Their subject is
/// namespaced so it can never equal a Google `sub` (Google subjects are digits only).
pub const GUEST_PREFIX: &str = "guest:";

impl User {
    pub fn is_guest(&self) -> bool {
        self.google_sub.starts_with(GUEST_PREFIX)
    }
}
