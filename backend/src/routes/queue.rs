//! Membership-bound queue request validation and handlers.
use crate::{api_error::ApiError, models::queue::valid_id, queue::QueueError};
use crate::{
    api_error::ErrorResponse,
    app_state::AppState,
    auth::AuthenticatedUser,
    models::queue::{MutationResponse, QueueView, SubmissionResponse, SubmitRequest},
};
use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    routing::{delete, get, post, put},
};
use axum::{
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/memberships/{membership_id}/queue", get(read))
        .route("/memberships/{membership_id}/queue/entries", post(submit))
        .route("/memberships/{membership_id}/queue/advance", post(advance))
        .route(
            "/memberships/{membership_id}/queue/entries/{entry_id}/move",
            post(move_entry),
        )
        .route(
            "/memberships/{membership_id}/queue/voting-policy",
            put(policy),
        )
        .route(
            "/memberships/{membership_id}/queue/entries/{entry_id}/votes",
            post(vote),
        )
        .route(
            "/memberships/{membership_id}/queue/entries/{entry_id}",
            delete(veto),
        )
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/queue/entries/{entry_id}/move", tag = "Queue",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path), ("entry_id" = String, Path)), request_body = crate::models::queue::MoveRequest,
    responses((status = 200, body = MutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse),
        (status = 413, body = ErrorResponse), (status = 415, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn move_entry(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<(String, String)>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<Json<MutationResponse>, QueueError> {
    let Path((membership, entry)) = path.map_err(|_| QueueError::InvalidRequest)?;
    id(&membership)?;
    id(&entry)?;
    let payload: crate::models::queue::MoveRequest = json(request).await?;
    let response = app
        .database
        .move_queue(&user, &membership, &entry, &payload, || (app.auth.clock)())
        .await?;
    publish(&app, response.changed, &response.queue);
    Ok(Json(response))
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/queue/entries/{entry_id}/votes", tag = "Queue",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path), ("entry_id" = String, Path)),
    responses((status = 200, body = MutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn vote(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<(String, String)>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<Json<MutationResponse>, QueueError> {
    let Path((membership, entry)) = path.map_err(|_| QueueError::InvalidRequest)?;
    id(&membership)?;
    id(&entry)?;
    bodyless(request).await?;
    let response = app
        .database
        .moderate_queue(
            &user,
            &membership,
            crate::database::queue::Moderation::Vote(&entry),
            || (app.auth.clock)(),
        )
        .await?;
    publish(&app, response.changed, &response.queue);
    Ok(Json(response))
}

#[utoipa::path(put, path = "/api/lobbies/memberships/{membership_id}/queue/voting-policy", tag = "Queue",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = crate::models::queue::PolicyRequest,
    responses((status = 200, body = MutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 413, body = ErrorResponse), (status = 415, body = ErrorResponse),
        (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn policy(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<Json<MutationResponse>, QueueError> {
    let Path(membership) = path.map_err(|_| QueueError::InvalidRequest)?;
    id(&membership)?;
    let payload: crate::models::queue::PolicyRequest = json(request).await?;
    payload.parts().map_err(|_| QueueError::InvalidRequest)?;
    let response = app
        .database
        .moderate_queue(
            &user,
            &membership,
            crate::database::queue::Moderation::Policy(&payload),
            || (app.auth.clock)(),
        )
        .await?;
    publish(&app, response.changed, &response.queue);
    Ok(Json(response))
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/queue/advance", tag = "Queue",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = crate::models::queue::AdvanceRequest,
    responses((status = 200, body = MutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 413, body = ErrorResponse),
        (status = 415, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn advance(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<Json<MutationResponse>, QueueError> {
    let Path(membership) = path.map_err(|_| QueueError::InvalidRequest)?;
    id(&membership)?;
    let payload: crate::models::queue::AdvanceRequest = json(request).await?;
    id(&payload.expected_entry_id)?;
    let _reason = payload.reason;
    let response = app
        .database
        .advance_queue(&user, &membership, &payload.expected_entry_id, || {
            (app.auth.clock)()
        })
        .await?;
    publish(&app, response.changed, &response.queue);
    Ok(Json(response))
}

#[utoipa::path(delete, path = "/api/lobbies/memberships/{membership_id}/queue/entries/{entry_id}", tag = "Queue",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path), ("entry_id" = String, Path)),
    responses((status = 200, body = MutationResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn veto(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<(String, String)>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<Json<MutationResponse>, QueueError> {
    let Path((membership, entry)) = path.map_err(|_| QueueError::InvalidRequest)?;
    id(&membership)?;
    id(&entry)?;
    bodyless(request).await?;
    let response = app
        .database
        .veto_queue(&user, &membership, &entry, || (app.auth.clock)())
        .await?;
    publish(&app, response.changed, &response.queue);
    Ok(Json(response))
}

#[utoipa::path(get, path = "/api/lobbies/memberships/{membership_id}/queue", tag = "Queue",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)),
    responses((status = 200, body = QueueView), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 500, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn read(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<Json<QueueView>, QueueError> {
    let Path(membership) = path.map_err(|_| QueueError::InvalidRequest)?;
    id(&membership)?;
    bodyless(request).await?;
    app.database
        .queue_view(&user, &membership, || (app.auth.clock)())
        .await
        .map(Json)
}

#[utoipa::path(post, path = "/api/lobbies/memberships/{membership_id}/queue/entries", tag = "Queue",
    security(("bearer_auth" = [])), params(("membership_id" = String, Path)), request_body = SubmitRequest,
    responses((status = 200, body = SubmissionResponse), (status = 400, body = ErrorResponse), (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 413, body = ErrorResponse),
        (status = 415, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse), (status = 503, body = ErrorResponse)))]
pub(super) async fn submit(
    State(app): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Result<Path<String>, axum::extract::rejection::PathRejection>,
    request: Request,
) -> Result<Json<SubmissionResponse>, QueueError> {
    let Path(membership) = path.map_err(|_| QueueError::InvalidRequest)?;
    id(&membership)?;
    let payload: SubmitRequest = json(request).await?;
    id(&payload.request_id)?;
    let response = crate::queue::submit(&app, &user, &membership, payload).await?;
    publish(&app, response.changed, &response.queue);
    Ok(Json(response))
}

fn publish(app: &AppState, changed: bool, view: &QueueView) {
    if changed {
        let bytes = base16ct::lower::decode_vec(&view.lobby_id).expect("stored lobby ID");
        app.lobby_updates.publish(
            crate::models::lobby::LobbyId::from_bytes(bytes.try_into().expect("128-bit lobby ID")),
            view.revision.value(),
        );
    }
}

pub(crate) fn id(value: &str) -> Result<(), QueueError> {
    valid_id(value)
        .then_some(())
        .ok_or(QueueError::InvalidRequest)
}

async fn json<T: DeserializeOwned>(request: Request) -> Result<T, QueueError> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime = content_type.split(';').next().unwrap_or("").trim();
    if !(mime.eq_ignore_ascii_case("application/json")
        || (mime.starts_with("application/") && mime.ends_with("+json")))
    {
        return Err(QueueError::MediaType);
    }
    let bytes = axum::body::to_bytes(request.into_body(), 4096)
        .await
        .map_err(|_| QueueError::TooLarge)?;
    serde_json::from_slice(&bytes).map_err(|_| QueueError::InvalidRequest)
}

async fn bodyless(request: Request) -> Result<(), QueueError> {
    axum::body::to_bytes(request.into_body(), 0)
        .await
        .map_err(|_| QueueError::InvalidRequest)?;
    Ok(())
}

impl IntoResponse for QueueError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => (401, "unauthorized", "Authentication required."),
            Self::MembershipForbidden => {
                (403, "membership_forbidden", "Active membership required.")
            }
            Self::HostRequired => (403, "host_required", "Host permission required."),
            Self::GuestRequired => (403, "guest_required", "Guest permission required."),
            Self::InvalidRequest => (400, "invalid_request", "Invalid queue request."),
            Self::TooLarge => (413, "request_too_large", "Queue request exceeds 4 KiB."),
            Self::MediaType => (415, "unsupported_media_type", "JSON content type required."),
            Self::EntryUnavailable => (404, "queue_entry_unavailable", "Queue entry unavailable."),
            Self::IdempotencyConflict => (
                409,
                "idempotency_conflict",
                "Retry identity has different content.",
            ),
            Self::CurrentChanged => (
                409,
                "current_entry_changed",
                "Current entry changed; refetch the queue.",
            ),
            Self::StaleRevision => (
                409,
                "stale_queue_revision",
                "Queue revision changed; refetch the queue.",
            ),
            Self::NotUpcoming => (409, "entry_not_upcoming", "Only upcoming entries may move."),
            Self::InvalidUrl => (
                422,
                "invalid_url",
                "A supported YouTube video URL is required.",
            ),
            Self::Upstream => (
                502,
                "upstream_unavailable",
                "Metadata service could not be reached.",
            ),
            Self::Metadata => (502, "metadata_unavailable", "Video metadata unavailable."),
            Self::Unavailable => (
                503,
                "temporarily_unavailable",
                "Queue temporarily unavailable.",
            ),
            Self::Database(error) => {
                eprintln!("queue storage failure: {error}");
                (500, "internal_error", "Queue operation failed.")
            }
        };
        let mut response =
            ApiError::new(StatusCode::from_u16(status).unwrap(), code, message).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        if status == 401 {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap());
        }
        response
    }
}
