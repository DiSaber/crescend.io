use super::AuthError;
use openidconnect::{CsrfToken, Nonce};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tokio::sync::Mutex;

pub const LOGIN_SECONDS: i64 = 600;

pub struct Attempt {
    pub nonce: Nonce,
    pub return_to: String,
    binding: [u8; 32],
    expires: i64,
}

pub struct Transactions {
    // Keyed by OAuth state; the lock makes binding validation and removal atomic.
    attempts: Mutex<HashMap<String, Attempt>>,
    capacity: usize,
}

impl Transactions {
    pub fn new(capacity: usize) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    pub async fn start(
        &self,
        now: i64,
        old_binding: Option<&str>,
        return_to: String,
    ) -> Result<(CsrfToken, Nonce, String), AuthError> {
        let mut attempts = self.attempts.lock().await;
        let old = old_binding.map(hash);
        // Reclaim expired entries and replace this browser's previous login attempt.
        attempts.retain(|_, a| a.expires > now && Some(a.binding) != old);
        if attempts.len() >= self.capacity {
            return Err(AuthError::Unavailable);
        }
        // State identifies the callback, nonce binds the ID token, binding identifies the browser.
        let state = CsrfToken::new_random();
        let nonce = Nonce::new_random();
        let binding = CsrfToken::new_random().secret().clone();
        attempts.insert(
            state.secret().clone(),
            Attempt {
                nonce: nonce.clone(),
                return_to,
                binding: hash(&binding),
                expires: now + LOGIN_SECONDS,
            },
        );
        Ok((state, nonce, binding))
    }

    pub async fn consume(
        &self,
        state: &str,
        binding: &str,
        now: i64,
    ) -> Result<Attempt, AuthError> {
        let mut attempts = self.attempts.lock().await;
        attempts.retain(|_, a| a.expires > now);
        // Check before removing so a wrong browser cannot consume another browser's attempt.
        let attempt = attempts.get(state).ok_or(AuthError::BadRequest)?;
        if attempt.binding != hash(binding) {
            return Err(AuthError::BadRequest);
        }
        attempts.remove(state).ok_or(AuthError::BadRequest)
    }
}

fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn attempts_are_bound_expiring_bounded_and_single_use() {
        let store = Transactions::new(1);
        let (state, _, binding) = store
            .start(100, None, "/first?x=1#part".into())
            .await
            .unwrap();
        assert!(
            store
                .start(100, None, "/first?x=1#part".into())
                .await
                .is_err()
        );
        assert!(store.consume(state.secret(), "wrong", 100).await.is_err());
        let (a, b) = tokio::join!(
            store.consume(state.secret(), &binding, 100),
            store.consume(state.secret(), &binding, 100)
        );
        assert_ne!(a.is_ok(), b.is_ok());
        assert_eq!(a.or(b).unwrap().return_to, "/first?x=1#part");
        let (state, _, binding) = store
            .start(100, None, "/first?x=1#part".into())
            .await
            .unwrap();
        assert!(store.consume(state.secret(), &binding, 700).await.is_err());
        let (old, _, old_binding) = store.start(700, None, "/old".into()).await.unwrap();
        let (new, _, new_binding) = store
            .start(700, Some(&old_binding), "/new".into())
            .await
            .unwrap();
        assert!(
            store
                .consume(old.secret(), &old_binding, 700)
                .await
                .is_err()
        );
        assert_eq!(
            store
                .consume(new.secret(), &new_binding, 700)
                .await
                .unwrap()
                .return_to,
            "/new"
        );
    }
}
