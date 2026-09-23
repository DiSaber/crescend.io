use url::Url;

// Deliberately no Debug: configuration contains credentials.
pub struct AuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub signing_secret: String,
    pub secure_cookie: bool,
}

impl AuthConfig {
    pub fn from_env() -> Result<Self, &'static str> {
        Self::load(|key| std::env::var(key).ok())
    }

    pub fn load(get: impl Fn(&str) -> Option<String>) -> Result<Self, &'static str> {
        let client_id = get("GOOGLE_CLIENT_ID")
            .filter(|s| !s.trim().is_empty())
            .ok_or("GOOGLE_CLIENT_ID is required")?;
        let client_secret = get("GOOGLE_CLIENT_SECRET")
            .filter(|s| !s.trim().is_empty())
            .ok_or("GOOGLE_CLIENT_SECRET is required")?;
        let redirect_uri = get("GOOGLE_REDIRECT_URI").ok_or("GOOGLE_REDIRECT_URI is required")?;
        let url = Url::parse(&redirect_uri).map_err(|_| "Invalid GOOGLE_REDIRECT_URI")?;
        let local_http = url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if (url.scheme() != "https" && !local_http)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/api/auth/google/callback"
        {
            return Err(
                "GOOGLE_REDIRECT_URI must be an HTTPS callback URL (HTTP is allowed only on localhost)",
            );
        }
        let signing_secret = get("JWT_SIGNING_SECRET")
            .filter(|s| s.len() >= 32)
            .ok_or("JWT_SIGNING_SECRET must contain at least 32 bytes")?;
        Ok(Self {
            client_id,
            client_secret,
            redirect_uri,
            signing_secret,
            secure_cookie: !local_http,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(key: &str) -> Option<String> {
        Some(
            match key {
                "GOOGLE_CLIENT_ID" => "test-client",
                "GOOGLE_CLIENT_SECRET" => "test-secret",
                "GOOGLE_REDIRECT_URI" => "https://example.com/api/auth/google/callback",
                "JWT_SIGNING_SECRET" => "test-signing-secret-at-least-32-bytes",
                _ => return None,
            }
            .into(),
        )
    }
    #[test]
    fn validates_required_configuration_without_exposing_values() {
        assert!(AuthConfig::load(config).unwrap().secure_cookie);
        for missing in [
            "GOOGLE_CLIENT_ID",
            "GOOGLE_CLIENT_SECRET",
            "GOOGLE_REDIRECT_URI",
            "JWT_SIGNING_SECRET",
        ] {
            assert!(AuthConfig::load(|k| if k == missing { None } else { config(k) }).is_err());
        }
        for uri in [
            "http://example.com/api/auth/google/callback",
            "https://example.com/wrong",
            "https://user:password@example.com/api/auth/google/callback",
            "https://example.com/api/auth/google/callback?next=evil",
            "not-a-url",
        ] {
            assert!(
                AuthConfig::load(|k| if k == "GOOGLE_REDIRECT_URI" {
                    Some(uri.into())
                } else {
                    config(k)
                })
                .is_err()
            );
        }
        assert!(
            !AuthConfig::load(|k| if k == "GOOGLE_REDIRECT_URI" {
                Some("http://localhost:3000/api/auth/google/callback".into())
            } else {
                config(k)
            })
            .unwrap()
            .secure_cookie
        );
        assert!(
            AuthConfig::load(|k| if k == "JWT_SIGNING_SECRET" {
                Some("too short".into())
            } else {
                config(k)
            })
            .is_err()
        );
    }
}
