use crate::{
    app_state::AppState,
    auth::{
        AuthConfig, AuthError, ErrorResponse, LOGIN_SECONDS,
        refresh::{self, TokenResponse},
    },
};
use axum::{
    Json, Router,
    extract::{RawQuery, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

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
    responses((status = 302, description = "Redirect to Google; sets a temporary HttpOnly login cookie"),
        (status = 503, description = "Login attempt capacity reached", body = ErrorResponse)))]
pub async fn start(State(state): State<AppState>, jar: CookieJar) -> Response {
    let result = state
        .auth
        .transactions
        .start((state.auth.clock)(), jar.get(COOKIE).map(|c| c.value()))
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

/// Complete Google login and return a crescend.io bearer JWT as JSON.
#[utoipa::path(get, path = "/api/auth/google/callback", tag = "Authentication", params(CallbackParams),
    responses((status = 200, description = "Login complete; JWT identifies the persisted user", body = TokenResponse),
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
        Ok::<_, AuthError>((params, attempt.nonce))
    }
    .await;
    let (params, nonce) = match validated {
        Ok(value) => value,
        Err(error) => return callback_response(error.into_response()),
    };
    // Every outcome after consumption clears the cookie, including denial and failures.
    let jar = jar.remove(cookie(&state.auth.config, String::new()));
    let result: Result<TokenResponse, AuthError> = async {
        if params.error.is_some() {
            return Err(AuthError::Unauthorized);
        }
        let subject = state
            .auth
            .google
            .authenticate(
                params.code.ok_or(AuthError::BadRequest)?,
                &nonce,
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
        refresh::issue(&state.database, &state.auth.jwt, user.id, &state.auth.clock).await
    }
    .await;
    let response = match result {
        Ok(token) => Json(token).into_response(),
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

#[derive(Deserialize, ToSchema)]
pub struct RefreshRequest {
    /// Current opaque refresh credential. Missing credentials are unauthorized.
    #[serde(default)]
    refresh_token: String,
}

/// Rotate an application refresh token without Google login or an access JWT.
#[utoipa::path(post, path = "/api/auth/refresh", tag = "Authentication",
    request_body = RefreshRequest,
    responses((status = 200, description = "Replacement credentials; session deadline is unchanged", body = TokenResponse),
        (status = 400, description = "Malformed JSON or non-string refresh token", body = ErrorResponse),
        (status = 401, description = "Missing, invalid, expired, revoked or reused refresh token", body = ErrorResponse),
        (status = 500, description = "Credential issuance or persistence failed", body = ErrorResponse)))]
pub async fn refresh_tokens(
    State(state): State<AppState>,
    body: Result<Json<RefreshRequest>, JsonRejection>,
) -> Response {
    let result = match body {
        Ok(Json(body)) => {
            refresh::rotate(
                &state.database,
                &state.auth.jwt,
                &body.refresh_token,
                &state.auth.clock,
            )
            .await
        }
        Err(_) => Err(AuthError::BadRequest),
    };
    let mut response = match result {
        Ok(tokens) => Json(tokens).into_response(),
        Err(error) => error.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
