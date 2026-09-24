use crate::{
    app_state::AppState,
    auth::{
        AuthConfig, AuthError, ErrorResponse, LOGIN_SECONDS, browser,
        refresh::{self, RefreshCredential, Rotation, TokenResponse},
    },
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{RawQuery, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use utoipa::IntoParams;

const COOKIE: &str = "crescend_google_login";
const COOKIE_PATH: &str = "/api/auth/google";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/start", get(start))
        .route("/callback", get(callback))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackParams {
    /// One-time login state, bound to the initiating browser.
    state: String,
    /// Authorization code returned by Google (absent on denial).
    code: Option<String>,
    /// Google authorization error (mutually exclusive with code).
    error: Option<String>,
}

fn cookie(config: &AuthConfig, value: String) -> Cookie<'static> {
    Cookie::build((COOKIE, value))
        .path(COOKIE_PATH)
        .http_only(true)
        .secure(config.secure_cookie)
        .same_site(SameSite::Lax)
        .max_age(
            std::time::Duration::from_secs(LOGIN_SECONDS as u64)
                .try_into()
                .expect("valid cookie lifetime"),
        )
        .build()
}

/// Redirect the browser to Google to sign in.
#[utoipa::path(get, path = "/api/auth/google/start", tag = "Authentication",
    params(("return_to" = Option<String>, Query, description = "Validated local destination; defaults to /; at most 2048 decoded bytes")),
    responses((status = 400, description = "Invalid return destination", body = ErrorResponse),
        (status = 302, description = "Redirect to Google; sets a temporary HttpOnly login cookie"),
        (status = 503, description = "Login attempt capacity reached", body = ErrorResponse)))]
pub async fn start(
    State(state): State<AppState>,
    jar: CookieJar,
    RawQuery(query): RawQuery,
) -> Response {
    let return_to = match crate::auth::destination::parse(query.as_deref()) {
        Ok(value) => value,
        Err(error) => return ([(header::CACHE_CONTROL, "no-store")], error).into_response(),
    };
    let result = state
        .auth
        .transactions
        .start(
            (state.auth.clock)(),
            jar.get(COOKIE).map(|c| c.value()),
            return_to,
        )
        .await;
    let (csrf, nonce, binding) = match result {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let url = state.auth.google.authorization_url(csrf, nonce);
    let jar = jar.add(cookie(&state.auth.config, binding));
    (
        StatusCode::FOUND,
        jar,
        [
            (header::LOCATION, url),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
    )
        .into_response()
}

fn parse_callback(query: Option<&str>) -> Result<CallbackParams, AuthError> {
    // Reject duplicate parameters before selecting state and exactly one of code/error.
    let mut values = std::collections::HashMap::new();
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        if values
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(AuthError::BadRequest);
        }
    }
    let state = values
        .remove("state")
        .filter(|s| !s.is_empty())
        .ok_or(AuthError::BadRequest)?;
    let code = values.remove("code");
    let error = values.remove("error");
    if code.is_some() == error.is_some()
        || code.as_ref().is_some_and(String::is_empty)
        || error.as_ref().is_some_and(String::is_empty)
    {
        return Err(AuthError::BadRequest);
    }
    Ok(CallbackParams { state, code, error })
}

/// Complete Google login, set the HttpOnly refresh cookie and redirect to the stored local page.
#[utoipa::path(get, path = "/api/auth/google/callback", tag = "Authentication", params(CallbackParams),
    responses((status = 303, description = "Session committed; sets refresh cookie, clears login cookie and redirects to stored return_to"),
        (status = 400, description = "Malformed, expired, mismatched or replayed callback", body = ErrorResponse),
        (status = 401, description = "Google login denied or credentials invalid", body = ErrorResponse),
        (status = 503, description = "Google authentication service unavailable", body = ErrorResponse),
        (status = 500, description = "Account persistence or token issuance failed", body = ErrorResponse)))]
pub async fn callback(
    State(state): State<AppState>,
    jar: CookieJar,
    RawQuery(query): RawQuery,
) -> Response {
    // Consume the browser-bound attempt before any Google request or account write.
    let validated = async {
        let params = parse_callback(query.as_deref())?;
        let binding = jar.get(COOKIE).ok_or(AuthError::BadRequest)?.value();
        let attempt = state
            .auth
            .transactions
            .consume(&params.state, binding, (state.auth.clock)())
            .await?;
        Ok::<_, AuthError>((params, attempt))
    }
    .await;
    let (params, attempt) = match validated {
        Ok(value) => value,
        Err(error) => return callback_response(error.into_response()),
    };
    // Every outcome after consumption clears the cookie, including denial and failures.
    let jar = jar.remove(cookie(&state.auth.config, String::new()));
    let result: Result<RefreshCredential, AuthError> = async {
        if params.error.is_some() {
            return Err(AuthError::Unauthorized);
        }
        let subject = state
            .auth
            .google
            .authenticate(
                params.code.ok_or(AuthError::BadRequest)?,
                &attempt.nonce,
                state.auth.clock.clone(),
            )
            .await?;
        // Only a verified Google subject can resolve an account; JWTs use its local ID.
        let user = match state.database.get_user_by_google_sub(&subject).await {
            Ok(user) => user,
            Err(sqlx::Error::RowNotFound) => {
                // A concurrent first login may win the insert; return 500 for now.
                state
                    .database
                    .create_user(&subject)
                    .await
                    .map_err(|_| AuthError::Internal)?
            }
            Err(_) => return Err(AuthError::Internal),
        };
        refresh::issue(&state.database, user.id, &state.auth.clock).await
    }
    .await;
    let response = match result {
        Ok(credential) => login_success(
            &state.auth.config,
            credential,
            &attempt.return_to,
            (state.auth.clock)(),
        ),
        Err(error) => error.into_response(),
    };
    callback_response((jar, response).into_response())
}

fn callback_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    response
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

fn cookie_response(cookie: Cookie<'static>, response: Response) -> Response {
    // Add an explicit removal cookie even when the incoming jar has no such cookie.
    (CookieJar::new().add(cookie), response).into_response()
}

fn cookie_error(config: &AuthConfig, error: AuthError) -> Response {
    let response = error.into_response();
    no_store(if error == AuthError::Unauthorized {
        cookie_response(browser::removal_cookie(config), response)
    } else {
        response
    })
}

fn login_success(
    config: &AuthConfig,
    credential: RefreshCredential,
    destination: &str,
    now: i64,
) -> Response {
    match browser::refresh_cookie(config, credential, now) {
        Ok(cookie) => cookie_response(
            cookie,
            (StatusCode::SEE_OTHER, [(header::LOCATION, destination)]).into_response(),
        ),
        Err(error) => cookie_error(config, error),
    }
}

fn refresh_response(
    config: &AuthConfig,
    result: Result<Rotation, AuthError>,
    now: i64,
) -> Response {
    let result = result.and_then(|rotation| {
        Ok((
            browser::refresh_cookie(config, rotation.credential, now)?,
            rotation.access,
        ))
    });
    match result {
        Ok((cookie, access)) => no_store(cookie_response(cookie, Json(access).into_response())),
        Err(error) => cookie_error(config, error),
    }
}

fn logout_response(config: &AuthConfig, result: Result<(), AuthError>) -> Response {
    match result {
        Ok(()) => no_store(cookie_response(
            browser::removal_cookie(config),
            StatusCode::NO_CONTENT.into_response(),
        )),
        Err(error) => cookie_error(config, error),
    }
}

// Check CSRF before even reading the body or cookies. Limit body collection to one byte:
// any nonempty body (including a legacy JSON payload) is invalid regardless of content type.
async fn cookie_request(config: &AuthConfig, request: Request) -> Result<HeaderMap, AuthError> {
    browser::csrf(config, request.headers())?;
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 1).await.map_err(|_| AuthError::BadRequest)?;
    if !body.is_empty() {
        return Err(AuthError::BadRequest);
    }
    Ok(parts.headers)
}

/// Bootstrap or renew access using only the rotating HttpOnly refresh cookie; send no body.
#[utoipa::path(post, path = "/api/auth/refresh", tag = "Authentication",
    params(("X-CSRF-Protection" = String, Header, description = "Required, exactly 1; same-origin requests only")),
    security(("refresh_cookie" = [])),
    responses((status = 200, description = "Access JWT and replacement refresh cookie; fixed session deadline", body = TokenResponse),
        (status = 400, description = "Nonempty request body; cookie unchanged", body = ErrorResponse),
        (status = 401, description = "Missing, ambiguous, invalid, expired, revoked or reused cookie; cookie cleared", body = ErrorResponse),
        (status = 403, description = "CSRF check failed; cookie unchanged", body = ErrorResponse),
        (status = 500, description = "Credential issuance or persistence failed; cookie unchanged", body = ErrorResponse)))]
pub async fn refresh_tokens(State(state): State<AppState>, request: Request) -> Response {
    let result = async {
        let headers = cookie_request(&state.auth.config, request).await?;
        let token = browser::refresh_value(&headers)?.ok_or(AuthError::Unauthorized)?;
        refresh::rotate(&state.database, &state.auth.jwt, &token, &state.auth.clock).await
    }
    .await;
    refresh_response(&state.auth.config, result, (state.auth.clock)())
}

/// Revoke this browser session; previously issued access JWTs retain their expiry.
#[utoipa::path(post, path = "/api/auth/logout", tag = "Authentication",
    params(("X-CSRF-Protection" = String, Header, description = "Required, exactly 1; same-origin requests only")),
    security(("refresh_cookie" = [])),
    responses((status = 204, description = "Session revoked or already absent; cookie cleared"),
        (status = 400, description = "Nonempty request body; cookie unchanged", body = ErrorResponse),
        (status = 401, description = "Duplicate cookies; cookie cleared", body = ErrorResponse),
        (status = 403, description = "CSRF check failed; cookie unchanged", body = ErrorResponse),
        (status = 500, description = "Revocation failed; cookie unchanged", body = ErrorResponse)))]
pub async fn logout(State(state): State<AppState>, request: Request) -> Response {
    let result = async {
        let headers = cookie_request(&state.auth.config, request).await?;
        if let Some(token) = browser::refresh_value(&headers)? {
            refresh::revoke(&state.database, &token).await?;
        }
        Ok(())
    }
    .await;
    logout_response(&state.auth.config, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use browser::test_config;

    fn config() -> AuthConfig {
        test_config("https://example.com/api/auth/google/callback")
    }

    fn credential() -> RefreshCredential {
        RefreshCredential {
            refresh_token: "private-refresh".into(),
            expires_at: 2000,
        }
    }

    fn rotation() -> Rotation {
        Rotation {
            credential: credential(),
            access: TokenResponse {
                access_token: "public-access".into(),
                token_type: "Bearer".into(),
                expires_in: 3600,
            },
        }
    }

    #[tokio::test]
    async fn callback_redirect_has_no_credentials_and_clears_login_cookie() {
        let config = config();
        // Match the original cookie jar supplied by a real request, not a newly added delta.
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, format!("{COOKIE}=binding").parse().unwrap());
        let original = CookieJar::from_headers(&headers).remove(cookie(&config, String::new()));
        let response = callback_response(
            (
                original,
                login_success(&config, credential(), "/lobbies?x=1#part", 1000),
            )
                .into_response(),
        );
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/lobbies?x=1#part");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(cookies.len(), 2);
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("crescend_refresh=private-refresh;")
                    && c.contains("HttpOnly"))
        );
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("crescend_google_login=;")
                    && c.contains("Max-Age=0")
                    && c.contains("Path=/api/auth/google"))
        );
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
        for error in [
            AuthError::BadRequest,
            AuthError::Unauthorized,
            AuthError::Unavailable,
            AuthError::Internal,
        ] {
            let original = CookieJar::from_headers(&headers).remove(cookie(&config, String::new()));
            let response = callback_response((original, error).into_response());
            assert_eq!(
                response
                    .headers()
                    .get_all(header::SET_COOKIE)
                    .iter()
                    .count(),
                1
            );
            assert!(
                !response.headers()[header::SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .contains("crescend_refresh")
            );
            assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        }
        let parsed =
            parse_callback(Some("state=state&code=code&return_to=https://evil.test")).unwrap();
        assert_eq!(parsed.state, "state"); // CallbackParams has no destination field.
    }

    #[tokio::test]
    async fn refresh_json_and_cookie_error_policy() {
        let config = config();
        let response = refresh_response(&config, Ok(rotation()), 1999);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("Max-Age=1"));
        let json: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "access_token": "public-access", "token_type": "Bearer", "expires_in": 3600 })
        );
        for now in [2000, 2001] {
            let response = refresh_response(&config, Ok(rotation()), now);
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(
                response.headers()[header::SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .contains("Max-Age=0")
            );
            assert!(
                !String::from_utf8(to_bytes(response.into_body(), 1024).await.unwrap().to_vec())
                    .unwrap()
                    .contains("public-access")
            );
        }
        for (error, status, clears) in [
            (AuthError::BadRequest, StatusCode::BAD_REQUEST, false),
            (AuthError::Forbidden, StatusCode::FORBIDDEN, false),
            (AuthError::Unauthorized, StatusCode::UNAUTHORIZED, true),
            (
                AuthError::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                false,
            ),
        ] {
            for response in [
                refresh_response(&config, Err(error), 1000),
                logout_response(&config, Err(error)),
            ] {
                assert_eq!(response.status(), status);
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
                assert_eq!(response.headers().contains_key(header::SET_COOKIE), clears);
            }
        }
    }

    #[tokio::test]
    async fn logout_success_clears_cookie_even_when_missing() {
        let response = logout_response(&config(), Ok(()));
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.starts_with("crescend_refresh=;"));
        assert!(cookie.contains("Path=/api/auth") && cookie.contains("Max-Age=0"));
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn request_policy_checks_csrf_before_body_or_cookie_processing() {
        let config = config();
        for body in ["", " ", "{", "{}", r#"{"refresh_token":"legacy"}"#] {
            for allowed in [false, true] {
                let mut request = Request::builder()
                    .header(header::COOKIE, "crescend_refresh=a; crescend_refresh=b");
                if allowed {
                    request = request.header("X-CSRF-Protection", "1");
                }
                let result = cookie_request(&config, request.body(Body::from(body)).unwrap()).await;
                let error = if !allowed {
                    Some(AuthError::Forbidden)
                } else if !body.is_empty() {
                    Some(AuthError::BadRequest)
                } else {
                    None
                };
                match error {
                    Some(error) => {
                        assert_eq!(result.unwrap_err(), error);
                        let response = cookie_error(&config, error);
                        assert!(!response.headers().contains_key(header::SET_COOKIE));
                    }
                    None => assert_eq!(
                        browser::refresh_value(&result.unwrap()),
                        Err(AuthError::Unauthorized)
                    ),
                }
            }
        }
        // Neither policy helper has access to persistence; callers mutate only after success.
    }
}
