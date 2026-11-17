//! Guest accounts: a nickname-only sign-in for joining lobbies.
//!
//! A guest is an ordinary `users` row whose subject is `guest:` + SHA-256(guest key).
//! The 32-byte guest key lives only in the browser's HttpOnly `crescend_guest` cookie,
//! so the same browser gets the same account back (votes, points, last nickname) even
//! after its refresh session ends. The database never holds the key itself.
use super::{AuthConfig, Clock};
use crate::models::user::GUEST_PREFIX;
use axum::http::{HeaderMap, header};
use axum_extra::extract::cookie::{Cookie, SameSite};
use rand::TryRng;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::Mutex,
};

pub const GUEST_COOKIE: &str = "crescend_guest";
const PATH: &str = "/api/auth";
/// Browsers cap cookie lifetimes at 400 days; every guest sign-in renews it.
const COOKIE_SECONDS: i64 = 400 * 24 * 60 * 60;
/// Set by Caddy from the resolved client address (see docs/guest-join.md).
pub const CLIENT_IP_HEADER: &str = "x-crescend-client-ip";

/// High enough for a class or party behind one shared public address (campus Wi-Fi,
/// carrier NAT); still stops one client from minting accounts without limit.
pub const NEW_GUESTS_PER_WINDOW: usize = 200;
pub const WINDOW_SECONDS: i64 = 60 * 60;

pub fn cookie(config: &AuthConfig, key: String) -> Cookie<'static> {
    Cookie::build((GUEST_COOKIE, key))
        .path(PATH)
        .http_only(true)
        .secure(config.secure_cookie)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(COOKIE_SECONDS))
        .build()
}

pub fn new_key() -> Option<String> {
    let mut random = [0u8; 32];
    rand::rngs::SysRng.try_fill_bytes(&mut random).ok()?;
    Some(base16ct::lower::encode_string(&random))
}

pub fn subject(key: &str) -> String {
    format!(
        "{GUEST_PREFIX}{}",
        base16ct::lower::encode_string(&Sha256::digest(key.as_bytes()))
    )
}

/// The browser's guest key when exactly one well-formed cookie is present. Anything
/// else (missing, duplicated, malformed) counts as a new browser.
pub fn key(headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for part in header.to_str().ok()?.split(';') {
            let (name, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
            if name.trim() == GUEST_COOKIE {
                if found.is_some() {
                    return None;
                }
                found = Some(value.trim().to_owned());
            }
        }
    }
    found.filter(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The address a request came from. Behind the front door the backend trusts only
/// the header Caddy overwrites on every request; without it (tests, direct runs) it
/// uses the TCP peer.
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    let mut values = headers.get_all(CLIENT_IP_HEADER).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => value.to_str().ok()?.trim().parse().ok(),
        (Some(_), Some(_)) => None,
        (None, _) => peer,
    }
}

/// IPv6 clients usually own a whole /64, so they share one bucket per /64.
fn bucket(ip: Option<IpAddr>) -> String {
    match ip.map(|ip| ip.to_canonical()) {
        Some(IpAddr::V4(v4)) => v4.to_string(),
        Some(IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        None => "unknown".into(),
    }
}

/// Sliding-window count of new guest accounts per client address, in memory: a
/// restart forgets it, which is fine for a nuisance limit.
pub struct GuestLimiter {
    clock: Clock,
    limit: usize,
    window: i64,
    created: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl GuestLimiter {
    pub fn new(clock: Clock) -> Self {
        Self::with_limit(clock, NEW_GUESTS_PER_WINDOW, WINDOW_SECONDS)
    }

    pub fn with_limit(clock: Clock, limit: usize, window: i64) -> Self {
        Self {
            clock,
            limit,
            window,
            created: Mutex::new(HashMap::new()),
        }
    }

    /// Records one creation, or returns the seconds until the next one is allowed.
    pub fn try_acquire(&self, ip: Option<IpAddr>) -> Result<(), i64> {
        let now = (self.clock)();
        let cutoff = now - self.window;
        let mut created = self.created.lock().unwrap_or_else(|e| e.into_inner());
        if created.len() > 10_000 {
            created.retain(|_, times| times.back().is_some_and(|t| *t > cutoff));
        }
        let times = created.entry(bucket(ip)).or_default();
        while times.front().is_some_and(|t| *t <= cutoff) {
            times.pop_front();
        }
        if times.len() >= self.limit {
            let oldest = *times.front().expect("full window");
            return Err((oldest + self.window - now).max(1));
        }
        times.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };

    #[test]
    fn limiter_counts_per_address_and_slides() {
        let time = Arc::new(AtomicI64::new(1000));
        let clock = time.clone();
        let limiter =
            GuestLimiter::with_limit(Arc::new(move || clock.load(Ordering::SeqCst)), 3, 100);
        let a: IpAddr = "203.0.113.7".parse().unwrap();
        let b: IpAddr = "203.0.113.8".parse().unwrap();
        for _ in 0..3 {
            assert_eq!(limiter.try_acquire(Some(a)), Ok(()));
        }
        assert_eq!(limiter.try_acquire(Some(a)), Err(100));
        assert_eq!(limiter.try_acquire(Some(b)), Ok(()));
        time.store(1050, Ordering::SeqCst);
        assert_eq!(limiter.try_acquire(Some(a)), Err(50));
        time.store(1100, Ordering::SeqCst);
        assert_eq!(limiter.try_acquire(Some(a)), Ok(()));
        // One /64 is one bucket; IPv4-mapped IPv6 is the IPv4 address.
        let v6 = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let limiter = GuestLimiter::with_limit(Arc::new(|| 0), 1, 100);
        assert!(limiter.try_acquire(v6("2001:db8:1:2::1")).is_ok());
        assert!(limiter.try_acquire(v6("2001:db8:1:2:ffff::9")).is_err());
        assert!(limiter.try_acquire(v6("2001:db8:1:3::1")).is_ok());
        assert!(limiter.try_acquire(v6("::ffff:198.51.100.1")).is_ok());
        assert!(limiter.try_acquire(v6("198.51.100.1")).is_err());
    }

    #[test]
    fn client_ip_prefers_the_front_door_header() {
        let peer: IpAddr = "172.18.0.3".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(client_ip(&headers, Some(peer)), Some(peer));
        headers.insert(CLIENT_IP_HEADER, "198.51.100.4".parse().unwrap());
        assert_eq!(client_ip(&headers, Some(peer)), "198.51.100.4".parse().ok());
        headers.append(CLIENT_IP_HEADER, "198.51.100.5".parse().unwrap());
        assert_eq!(client_ip(&headers, Some(peer)), None);
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_IP_HEADER, "not an ip".parse().unwrap());
        assert_eq!(client_ip(&headers, Some(peer)), None);
    }

    #[test]
    fn guest_keys_are_strict_and_subjects_hash_them() {
        let key_a = "a".repeat(64);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("x=1; {GUEST_COOKIE}={key_a}").parse().unwrap(),
        );
        assert_eq!(key(&headers), Some(key_a.clone()));
        headers.append(
            header::COOKIE,
            format!("{GUEST_COOKIE}={key_a}").parse().unwrap(),
        );
        assert_eq!(key(&headers), None);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{GUEST_COOKIE}=short").parse().unwrap(),
        );
        assert_eq!(key(&headers), None);
        let subject = subject(&key_a);
        assert!(subject.starts_with("guest:") && subject.len() == 6 + 64);
        assert!(!subject.contains(&key_a));
        assert_eq!(new_key().unwrap().len(), 64);
    }
}
