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
    paths(lobbies::create_lobby, lobbies::join_lobby, youtube::metadata, auth::start, auth::callback, auth::refresh_tokens, auth::logout),
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
        .route_layer(AsyncRequireAuthorizationLayer::new(jwt))
        .layer(axum::middleware::map_response(lobby_no_store));
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

async fn lobby_no_store(mut response: axum::response::Response) -> axum::response::Response {
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod lobby_documentation_tests {
    use super::*;

    #[test]
    fn only_create_and_join_are_documented_with_the_response_contract() {
        let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let paths = document["paths"].as_object().unwrap();
        let lobby_paths: Vec<_> = paths
            .keys()
            .filter(|p| p.starts_with("/api/lobbies"))
            .collect();
        assert_eq!(lobby_paths, ["/api/lobbies", "/api/lobbies/join"]);
        let join = &paths["/api/lobbies/join"]["post"];
        for status in [
            "200", "400", "401", "404", "409", "413", "415", "500", "503",
        ] {
            assert!(
                join["responses"][status].is_object(),
                "missing join {status}"
            );
        }
        assert_eq!(
            join["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/JoinLobbyRequest"
        );
        let operation = &paths["/api/lobbies"]["post"];
        assert!(operation["requestBody"].is_null());
        assert!(operation["responses"]["200"].is_null());
        for status in ["201", "400", "401", "409", "500", "503"] {
            assert!(
                operation["responses"][status].is_object(),
                "missing {status}"
            );
        }
        assert_eq!(
            operation["responses"]["201"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/CurrentLobby"
        );
        let schemas = document["components"]["schemas"].as_object().unwrap();
        for name in [
            "CurrentLobby",
            "LobbyView",
            "MemberView",
            "MemberRole",
            "LobbyId",
            "MembershipId",
            "JoinLobbyRequest",
        ] {
            assert!(schemas.contains_key(name), "missing schema {name}");
        }
    }
}
