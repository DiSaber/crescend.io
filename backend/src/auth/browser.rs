use super::{AuthConfig, AuthError, refresh::RefreshCredential};
use axum::http::{HeaderMap, header};
use axum_extra::extract::cookie::{Cookie, SameSite};
use time::{Duration, OffsetDateTime};
use url::Url;

pub const REFRESH_COOKIE: &str = "crescend_refresh";
const PATH: &str = "/api/auth";

fn scoped_cookie(config: &AuthConfig, value: String) -> Cookie<'static> {
    Cookie::build((REFRESH_COOKIE, value))
        .path(PATH)
        .http_only(true)
        .secure(config.secure_cookie)
        .same_site(SameSite::Lax)
        .build()
}

pub fn refresh_cookie(
    config: &AuthConfig,
    credential: RefreshCredential,
    now: i64,
) -> Result<Cookie<'static>, AuthError> {
    let remaining = credential
        .expires_at
        .checked_sub(now)
        .filter(|s| *s > 0)
        .ok_or(AuthError::Unauthorized)?;
    let expires = OffsetDateTime::from_unix_timestamp(credential.expires_at)
        .map_err(|_| AuthError::Internal)?;
    let mut cookie = scoped_cookie(config, credential.refresh_token);
    cookie.set_expires(expires);
    cookie.set_max_age(Duration::seconds(remaining));
    Ok(cookie)
}

pub fn removal_cookie(config: &AuthConfig) -> Cookie<'static> {
    let mut cookie = scoped_cookie(config, String::new());
    cookie.make_removal();
    cookie
}

// Do not use CookieJar here: it collapses duplicate names, including across header lines.
pub fn refresh_value(headers: &HeaderMap) -> Result<Option<String>, AuthError> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        let value = header.to_str().map_err(|_| AuthError::Unauthorized)?;
        for part in value.split(';') {
            let (name, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
            if name.trim() == REFRESH_COOKIE {
                if found.is_some() {
                    return Err(AuthError::Unauthorized);
                }
                found = Some(value.trim().to_owned());
            }
        }
    }
    Ok(found)
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, AuthError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(AuthError::Forbidden);
    }
    first
        .map(|v| v.to_str().map_err(|_| AuthError::Forbidden))
        .transpose()
}

pub fn csrf(config: &AuthConfig, headers: &HeaderMap) -> Result<(), AuthError> {
    if single_header(headers, "x-csrf-protection")? != Some("1") {
        return Err(AuthError::Forbidden);
    }
    if let Some(origin) = single_header(headers, "origin")? {
        let parsed = Url::parse(origin).map_err(|_| AuthError::Forbidden)?;
        let trusted = Url::parse(&config.redirect_uri).map_err(|_| AuthError::Internal)?;
        // Origin is an origin serialization, never a URL with userinfo, path, query or fragment.
        let authority = origin
            .split_once("://")
            .map(|(_, rest)| rest)
            .ok_or(AuthError::Forbidden)?;
        if authority.contains(['/', '\\', '?', '#', '@'])
            || origin.chars().any(|c| c.is_control() || c.is_whitespace())
            || !parsed.origin().is_tuple()
            || parsed.origin() != trusted.origin()
        {
            return Err(AuthError::Forbidden);
        }
    }
    if let Some(site) = single_header(headers, "sec-fetch-site")?
        && site != "same-origin"
    {
        return Err(AuthError::Forbidden);
    }
    Ok(())
}

#[cfg(test)]
pub fn test_config(uri: &str) -> AuthConfig {
    AuthConfig::load(|key| {
        Some(
            match key {
                "GOOGLE_CLIENT_ID" => "test-client",
                "GOOGLE_CLIENT_SECRET" => "test-secret",
                "GOOGLE_REDIRECT_URI" => uri,
                "JWT_SIGNING_SECRET" => "test-secret-at-least-thirty-two-bytes",
                _ => return None,
            }
            .into(),
        )
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_have_fixed_expiry_and_matching_removal_scope() {
        for uri in [
            "https://example.com/api/auth/google/callback",
            "http://localhost:3000/api/auth/google/callback",
        ] {
            let config = test_config(uri);
            let cookie = refresh_cookie(
                &config,
                RefreshCredential {
                    refresh_token: "secret".into(),
                    expires_at: 2000,
                },
                1999,
            )
            .unwrap();
            assert_eq!(cookie.name(), REFRESH_COOKIE);
            assert_eq!(cookie.value(), "secret");
            assert_eq!(cookie.path(), Some(PATH));
            assert_eq!(cookie.domain(), None);
            assert_eq!(cookie.http_only(), Some(true));
            assert_eq!(cookie.secure(), Some(config.secure_cookie));
            assert_eq!(cookie.same_site(), Some(SameSite::Lax));
            assert_eq!(cookie.expires_datetime().unwrap().unix_timestamp(), 2000);
            assert_eq!(cookie.max_age().unwrap().whole_seconds(), 1);
            let removal = removal_cookie(&config);
            assert_eq!(removal.name(), cookie.name());
            assert_eq!(removal.path(), cookie.path());
            assert_eq!(removal.domain(), cookie.domain());
            assert_eq!(removal.max_age(), Some(Duration::ZERO));
            for now in [2000, 2001] {
                assert!(matches!(
                    refresh_cookie(
                        &config,
                        RefreshCredential {
                            refresh_token: "secret".into(),
                            expires_at: 2000
                        },
                        now
                    ),
                    Err(AuthError::Unauthorized)
                ));
            }
        }
    }

    #[test]
    fn cookie_extraction_rejects_ambiguity() {
        for (values, expected) in [
            (vec![], Ok(None)),
            (vec!["other=x"], Ok(None)),
            (vec!["crescend_refresh="], Ok(Some("".into()))),
            (
                vec!["other=x; crescend_refresh=one"],
                Ok(Some("one".into())),
            ),
            (
                vec!["crescend_refresh=one; crescend_refresh=two"],
                Err(AuthError::Unauthorized),
            ),
            (
                vec!["crescend_refresh=one", "crescend_refresh=one"],
                Err(AuthError::Unauthorized),
            ),
            (
                vec!["crescend_refresh; crescend_refresh=one"],
                Err(AuthError::Unauthorized),
            ),
        ] {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(header::COOKIE, value.parse().unwrap());
            }
            assert_eq!(refresh_value(&headers), expected);
        }
    }

    #[test]
    fn csrf_policy_is_same_origin_and_unambiguous() {
        let config = test_config("https://example.com/api/auth/google/callback");
        for (entries, allowed) in [
            (vec![], false),
            (vec![("x-csrf-protection", "1")], true),
            (vec![("x-csrf-protection", "0")], false),
            (
                vec![("x-csrf-protection", "1"), ("x-csrf-protection", "1")],
                false,
            ),
            (vec![("x-csrf-protection", "1, 1")], false),
            (
                vec![
                    ("x-csrf-protection", "1"),
                    ("origin", "https://example.com:443"),
                    ("sec-fetch-site", "same-origin"),
                ],
                true,
            ),
            (
                vec![
                    ("x-csrf-protection", "1"),
                    ("origin", "https://EXAMPLE.com"),
                ],
                true,
            ),
        ] {
            let mut headers = HeaderMap::new();
            for (key, value) in entries {
                headers.append(key, value.parse().unwrap());
            }
            assert_eq!(csrf(&config, &headers).is_ok(), allowed);
        }
        for origin in [
            "null",
            "http://example.com",
            "https://example.com:444",
            "https://other.example.com",
            "https://example.com/",
            "https://example.com/path",
            "https://user@example.com",
            "https://example.com?x",
            "https://example.com#x",
            "https://example.com https://example.com",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("x-csrf-protection", "1".parse().unwrap());
            headers.insert("origin", origin.parse().unwrap());
            assert_eq!(
                csrf(&config, &headers),
                Err(AuthError::Forbidden),
                "{origin}"
            );
        }
        for name in ["origin", "sec-fetch-site"] {
            let good = if name == "origin" {
                "https://example.com"
            } else {
                "same-origin"
            };
            let mut headers = HeaderMap::new();
            headers.insert("x-csrf-protection", "1".parse().unwrap());
            headers.append(name, good.parse().unwrap());
            headers.append(name, good.parse().unwrap());
            assert_eq!(csrf(&config, &headers), Err(AuthError::Forbidden));
        }
        for site in [
            "same-site",
            "cross-site",
            "none",
            "same-origin, same-origin",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("x-csrf-protection", "1".parse().unwrap());
            headers.insert("sec-fetch-site", site.parse().unwrap());
            assert_eq!(csrf(&config, &headers), Err(AuthError::Forbidden));
        }
        let local = test_config("http://localhost:3000/api/auth/google/callback");
        let mut headers = HeaderMap::new();
        headers.insert("x-csrf-protection", "1".parse().unwrap());
        headers.insert("origin", "http://localhost:3000".parse().unwrap());
        assert!(csrf(&local, &headers).is_ok());
        headers.insert("origin", "http://localhost".parse().unwrap());
        assert!(csrf(&local, &headers).is_err());
    }
}
