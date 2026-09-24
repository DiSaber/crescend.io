mod auth;
mod lobbies;
mod youtube;

use crate::auth::JwtAuth;
use axum::Router;
use tower_http::auth::AsyncRequireAuthorizationLayer;
use utoipa::{Modify, OpenApi};
use utoipa_scalar::{Scalar, Servable};

use crate::app_state::AppState;

#[derive(OpenApi)]
#[openapi(
    info(title = "Crescend.io API"),
    paths(lobbies::create_lobby, youtube::metadata, auth::start, auth::callback, auth::refresh_tokens, auth::logout),
    modifiers(&Security),
    tags(
        (name = "Authentication", description = "Google login"),
        (name = "Lobbies", description = "Lobby endpoints"),
        (name = "YouTube", description = "YouTube endpoints")
    )
)]
struct ApiDoc;

struct Security;
impl Modify for Security {
    fn modify(&self, doc: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{
            ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme,
        };
        doc.components
            .as_mut()
            .expect("OpenAPI components")
            .add_security_scheme(
                "refresh_cookie",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("crescend_refresh"))),
            );
        doc.components
            .as_mut()
            .expect("OpenAPI components")
            .add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
    }
}

pub fn router(jwt: JwtAuth) -> Router<AppState> {
    let protected = Router::new()
        .nest("/api/lobbies", lobbies::router())
        .route_layer(AsyncRequireAuthorizationLayer::new(jwt));
    Router::new()
        .merge(Scalar::with_url("/scalar", ApiDoc::openapi()))
        .merge(protected)
        .nest("/api/auth/google", auth::router())
        .route(
            "/api/auth/refresh",
            axum::routing::post(auth::refresh_tokens),
        )
        .route("/api/auth/logout", axum::routing::post(auth::logout))
        .nest("/api/youtube", youtube::router())
}
