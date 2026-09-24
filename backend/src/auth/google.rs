use super::{AuthConfig, AuthError, Clock};
use openidconnect::{
    AsyncHttpClient, AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EndpointNotSet, EndpointSet, IssuerUrl, JsonWebKey, Nonce, RedirectUrl, RequestTokenError,
    TokenResponse, TokenUrl,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreIdToken, CoreIdTokenVerifier, CoreJsonWebKeySet,
        CoreJwsSigningAlgorithm,
    },
};
use std::time::Duration;
use tokio::sync::Mutex;

const ISSUER: &str = "https://accounts.google.com";
type Client = CoreClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

pub struct Google {
    client: Client,
    http: reqwest::Client,
    client_id: ClientId,
    keys_url: String,
    cache: Mutex<KeyCache>,
}

#[derive(Default)]
struct KeyCache {
    keys: Option<CoreJsonWebKeySet>,
    expires: i64,
    last_refresh: Option<i64>,
    failed: bool,
}

impl Google {
    pub fn new(config: &AuthConfig) -> Result<Self, &'static str> {
        Self::with_endpoints(
            config,
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "https://www.googleapis.com/oauth2/v3/certs",
        )
    }

    // Endpoint injection is private to this module and its tests; production URLs are fixed.
    pub(super) fn with_endpoints(
        config: &AuthConfig,
        auth_url: &str,
        token_url: &str,
        keys_url: &str,
    ) -> Result<Self, &'static str> {
        let client_id = ClientId::new(config.client_id.clone());
        let client = CoreClient::new(
            client_id.clone(),
            IssuerUrl::new(ISSUER.into()).map_err(|_| "Invalid issuer")?,
            CoreJsonWebKeySet::new(vec![]),
        )
        .set_client_secret(ClientSecret::new(config.client_secret.clone()))
        .set_auth_uri(AuthUrl::new(auth_url.into()).map_err(|_| "Invalid authorization URL")?)
        .set_token_uri(TokenUrl::new(token_url.into()).map_err(|_| "Invalid token URL")?)
        .set_redirect_uri(
            RedirectUrl::new(config.redirect_uri.clone()).map_err(|_| "Invalid callback URL")?,
        )
        .set_auth_type(AuthType::RequestBody);
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "Could not create Google HTTP client")?;
        Ok(Self {
            client,
            http,
            client_id,
            keys_url: keys_url.into(),
            cache: Mutex::new(KeyCache::default()),
        })
    }

    pub fn authorization_url(&self, state: CsrfToken, nonce: Nonce) -> String {
        self.client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                || state,
                || nonce,
            )
            .url()
            .0
            .into()
    }

    pub async fn authenticate(
        &self,
        code: String,
        nonce: &Nonce,
        clock: Clock,
    ) -> Result<String, AuthError> {
        let http = TokenHttp(&self.http);
        let response = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .request_async(&http)
            .await
            .map_err(|error| match error {
                RequestTokenError::ServerResponse(_) => AuthError::Unauthorized,
                RequestTokenError::Parse(_, _) => AuthError::Unauthorized,
                _ => AuthError::Unavailable,
            })?;
        let token = response.id_token().ok_or(AuthError::Unauthorized)?;
        self.verify(token, nonce, clock).await
    }

    async fn verify(
        &self,
        token: &CoreIdToken,
        nonce: &Nonce,
        clock: Clock,
    ) -> Result<String, AuthError> {
        if *token.signing_alg().map_err(|_| AuthError::Unauthorized)?
            != CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256
        {
            return Err(AuthError::Unauthorized);
        }
        // Read only the routing hint here; no claim is trusted before OIDC verification.
        let header =
            jsonwebtoken::decode_header(token.to_string()).map_err(|_| AuthError::Unauthorized)?;
        let keys = self.keys(header.kid.as_deref(), (clock)()).await?;
        // Read time after network work so a token that expired while waiting is rejected.
        let time = chrono::DateTime::from_timestamp((clock)(), 0).ok_or(AuthError::Internal)?;
        let verifier = CoreIdTokenVerifier::new_public_client(
            self.client_id.clone(),
            IssuerUrl::new(ISSUER.into()).map_err(|_| AuthError::Internal)?,
            keys,
        )
        .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
        .set_time_fn(move || time)
        // Additional audiences are permitted only with our client as the authorized party below.
        .set_other_audience_verifier_fn(|_| true);
        let claims = token
            .claims(&verifier, nonce)
            .map_err(|_| AuthError::Unauthorized)?;
        if claims.subject().as_str().is_empty()
            || claims
                .authorized_party()
                .is_some_and(|party| party != &self.client_id)
            || (claims.audiences().len() > 1 && claims.authorized_party() != Some(&self.client_id))
        {
            return Err(AuthError::Unauthorized);
        }
        Ok(claims.subject().as_str().to_owned())
    }

    async fn keys(&self, kid: Option<&str>, now: i64) -> Result<CoreJsonWebKeySet, AuthError> {
        // Holding the mutex during the bounded fetch coalesces concurrent refreshes.
        let mut cache = self.cache.lock().await;
        let matches = |keys: &CoreJsonWebKeySet| {
            kid.is_none()
                || keys
                    .keys()
                    .iter()
                    .any(|key| key.key_id().map(|id| id.as_str()) == kid)
        };
        if cache.expires > now
            && let Some(keys) = &cache.keys
            && matches(keys)
        {
            return Ok(keys.clone());
        }
        if cache
            .last_refresh
            .is_some_and(|last| now.saturating_sub(last) < 5)
        {
            return Err(if cache.failed || cache.expires <= now {
                AuthError::Unavailable
            } else {
                AuthError::Unauthorized
            });
        }
        cache.last_refresh = Some(now);
        cache.failed = true;
        let response = self
            .http
            .get(&self.keys_url)
            .send()
            .await
            .map_err(|_| AuthError::Unavailable)?;
        if !response.status().is_success() {
            return Err(AuthError::Unavailable);
        }
        let ttl = cache_ttl(response.headers());
        let keys: CoreJsonWebKeySet = response.json().await.map_err(|_| AuthError::Unavailable)?;
        cache.expires = now.saturating_add(ttl);
        cache.failed = false;
        cache.keys = Some(keys.clone());
        if !matches(&keys) {
            return Err(AuthError::Unauthorized);
        }
        Ok(keys)
    }
}

fn cache_ttl(headers: &reqwest::header::HeaderMap) -> i64 {
    let control = headers
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let directives: Vec<_> = control.split(',').map(str::trim).collect();
    if directives
        .iter()
        .any(|d| d.eq_ignore_ascii_case("no-store") || d.eq_ignore_ascii_case("no-cache"))
    {
        return 0;
    }
    let max_age = directives
        .iter()
        .find_map(|d| {
            let (key, value) = d.split_once('=')?;
            key.eq_ignore_ascii_case("max-age")
                .then(|| value.trim_matches('"').parse::<i64>().ok())
                .flatten()
        })
        .unwrap_or(0)
        .max(0);
    let age = headers
        .get(reqwest::header::AGE)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    max_age.saturating_sub(age).max(0)
}

// Preserve transport/provider outages as 503 instead of confusing them with invalid credentials.
struct TokenHttp<'a>(&'a reqwest::Client);
impl<'c> AsyncHttpClient<'c> for TokenHttp<'_> {
    type Error = std::io::Error;
    type Future = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<openidconnect::HttpResponse, Self::Error>>
                + Send
                + Sync
                + 'c,
        >,
    >;
    fn call(&'c self, request: openidconnect::HttpRequest) -> Self::Future {
        Box::pin(async move {
            let request = reqwest::Request::try_from(request)
                .map_err(|_| std::io::Error::other("Invalid Google request"))?;
            let response = self
                .0
                .execute(request)
                .await
                .map_err(|_| std::io::Error::other("Google request failed"))?;
            if response.status().is_server_error() || response.status().as_u16() == 429 {
                return Err(std::io::Error::other("Google temporarily unavailable"));
            }
            let mut result = openidconnect::http::Response::builder()
                .status(response.status())
                .version(response.version());
            *result.headers_mut().expect("valid response builder") = response.headers().clone();
            let body = response
                .bytes()
                .await
                .map_err(|_| std::io::Error::other("Google response failed"))?;
            result
                .body(body.to_vec())
                .map_err(|_| std::io::Error::other("Invalid Google response"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn respects_key_cache_directives_and_age() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(cache_ttl(&headers), 0);
        headers.insert("cache-control", "public, max-age=3600".parse().unwrap());
        headers.insert("age", "100".parse().unwrap());
        assert_eq!(cache_ttl(&headers), 3500);
        for value in [
            "max-age=3600, no-store",
            "no-cache, max-age=3600",
            "max-age=invalid",
            "max-age=-1",
        ] {
            headers.insert("cache-control", value.parse().unwrap());
            assert_eq!(cache_ttl(&headers), 0);
        }
    }
}
