//! Authenticated song search HTTP boundary.
use crate::{api_error::ApiError, app_state::AppState, models::search::SearchResponse};
use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
};
fn invalid() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "invalid_search_request",
        "Invalid song search request.",
    )
}
fn decode(value: &str) -> Result<String, ApiError> {
    let mut bytes = Vec::new();
    let mut input = value.bytes();
    while let Some(b) = input.next() {
        match b {
            b'%' => {
                let a = input
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .ok_or_else(invalid)?;
                let b = input
                    .next()
                    .and_then(|v| (v as char).to_digit(16))
                    .ok_or_else(invalid)?;
                bytes.push((a * 16 + b) as u8);
            }
            b'+' => bytes.push(b' '),
            _ => bytes.push(b),
        }
    }
    String::from_utf8(bytes).map_err(|_| invalid())
}
#[utoipa::path(get, path="/api/songs/search", tag="Songs", security(("bearer_auth"=[])),
    params(("q"=String, Query, description="1–200 Unicode scalar values"),("more"=Option<bool>, Query)),
    responses((status=200,body=SearchResponse),(status=400,body=crate::api_error::ErrorResponse),(status=401,body=crate::api_error::ErrorResponse),(status=414,body=crate::api_error::ErrorResponse),(status=503,body=crate::api_error::ErrorResponse),(status=500,body=crate::api_error::ErrorResponse)))]
pub async fn search(
    State(app): State<AppState>,
    request: Request,
) -> Result<Json<SearchResponse>, ApiError> {
    let raw = request.uri().query().unwrap_or_default();
    if raw.len() > 8192 {
        return Err(ApiError::new(
            StatusCode::URI_TOO_LONG,
            "search_uri_too_long",
            "Song search URI is too long.",
        ));
    }
    let mut q = None;
    let mut more = None;
    for pair in raw.split('&') {
        let (key, value) = pair.split_once('=').ok_or_else(invalid)?;
        let key = decode(key)?;
        let value = decode(value)?;
        match key.as_str() {
            "q" if q.is_none() => {
                q = Some(crate::search::normalize::query(&value).ok_or_else(invalid)?)
            }
            "more" if more.is_none() => {
                more = Some(match value.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err(invalid()),
                })
            }
            _ => return Err(invalid()),
        }
    }
    let q = q.ok_or_else(invalid)?;
    // Bound body collection; authentication has already run in the protected router.
    if !axum::body::to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| invalid())?
        .is_empty()
    {
        return Err(invalid());
    }
    app.search
        .search(&app.database, &q, more.unwrap_or(false), (app.auth.clock)())
        .await
        .map(Json)
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "Song search temporarily unavailable.",
            )
        })
}
