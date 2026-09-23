use super::*;
use crate::database::Database;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::{
    OnceLock,
    atomic::{AtomicI64, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};
const NOW: i64 = 1_800_000_000;
const SECRET: &str = "test-only-application-signing-secret-32-bytes";
struct SigningKey {
    encoding: EncodingKey,
    jwk: Value,
    kid: &'static str,
}
impl SigningKey {
    fn new(kid: &'static str) -> Self {
        let key = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let jwk = json!({"kty":"RSA","use":"sig","alg":"RS256","kid":kid,
            "n":URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()), "e":URL_SAFE_NO_PAD.encode(key.e().to_bytes_be())});
        Self {
            encoding: EncodingKey::from_rsa_der(key.to_pkcs1_der().unwrap().as_bytes()),
            jwk,
            kid,
        }
    }
    fn token(&self, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.kid.into());
        encode(&header, claims, &self.encoding).unwrap()
    }
}
fn key() -> &'static SigningKey {
    static KEY: OnceLock<SigningKey> = OnceLock::new();
    KEY.get_or_init(|| SigningKey::new("first"))
}
fn second_key() -> &'static SigningKey {
    static KEY: OnceLock<SigningKey> = OnceLock::new();
    KEY.get_or_init(|| SigningKey::new("second"))
}

struct Fixture {
    google: google::Google,
    server: MockServer,
    time: Arc<AtomicI64>,
}
impl Fixture {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let config = AuthConfig::load(|k| {
            Some(
                match k {
                    "GOOGLE_CLIENT_ID" => "test-client",
                    "GOOGLE_CLIENT_SECRET" => "test-client-secret",
                    "GOOGLE_REDIRECT_URI" => "https://example.com/api/auth/google/callback",
                    "JWT_SIGNING_SECRET" => SECRET,
                    _ => return None,
                }
                .into(),
            )
        })
        .unwrap();
        let google = google::Google::with_endpoints(
            &config,
            "https://accounts.google.com/o/oauth2/v2/auth",
            &format!("{}/token", server.uri()),
            &format!("{}/keys", server.uri()),
        )
        .unwrap();
        Self {
            google,
            server,
            time: Arc::new(AtomicI64::new(NOW)),
        }
    }
    async fn authenticate(&self, code: &str) -> Result<String, AuthError> {
        let time = self.time.clone();
        self.google
            .authenticate(
                code.into(),
                &openidconnect::Nonce::new("nonce".into()),
                Arc::new(move || time.load(Ordering::SeqCst)),
            )
            .await
    }
    async fn token(&self, code: &str, jwt: &str) {
        Mock::given(method("POST")).and(path("/token")).and(body_string_contains(format!("code={code}")))
            .and(body_string_contains("grant_type=authorization_code")).and(body_string_contains("client_id=test-client"))
            .and(body_string_contains("client_secret=test-client-secret"))
            .and(body_string_contains("redirect_uri=https%3A%2F%2Fexample.com%2Fapi%2Fauth%2Fgoogle%2Fcallback"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"unused-google-access-token", "token_type":"Bearer", "id_token":jwt}))).mount(&self.server).await;
    }
    async fn keys(&self, key: &SigningKey) {
        Mock::given(method("GET"))
            .and(path("/keys"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "public, max-age=3600")
                    .set_body_json(json!({"keys":[key.jwk]})),
            )
            .mount(&self.server)
            .await;
    }
    async fn requests(&self, path: &str) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == path)
            .count()
    }
}
fn claims() -> Value {
    json!({"iss":"https://accounts.google.com", "sub":"google-user", "aud":"test-client", "iat":NOW, "exp":NOW+3600, "nonce":"nonce"})
}
#[tokio::test]
async fn invalid_google_claims_signatures_and_algorithms_issue_no_jwt() {
    let f = Fixture::new().await;
    f.keys(key()).await;
    f.token("valid", &key().token(&claims())).await;
    assert_eq!(f.authenticate("valid").await.unwrap(), "google-user");
    let mutations = [
        ("iss", json!("https://evil.example")),
        ("iss", json!("accounts.google.com")),
        ("aud", json!("another-client")),
        ("exp", json!(NOW)),
        ("sub", json!("")),
        ("nonce", json!("wrong")),
        ("azp", json!("other")),
        ("aud", json!(["test-client", "other"])),
        ("exp", Value::Null),
        ("sub", Value::Null),
        ("nonce", Value::Null),
        ("iss", Value::Null),
        ("aud", Value::Null),
    ];
    for (i, (field, value)) in mutations.into_iter().enumerate() {
        let mut c = claims();
        if value.is_null() {
            c.as_object_mut().unwrap().remove(field);
        } else {
            c[field] = value;
        }
        let code = format!("invalid{i}");
        f.token(&code, &key().token(&c)).await;
        assert_eq!(
            f.authenticate(&code).await.unwrap_err(),
            AuthError::Unauthorized,
            "accepted {field}"
        );
    }
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(key().kid.into());
    let forged = encode(&header, &claims(), &second_key().encoding).unwrap();
    f.token("forged", &forged).await;
    assert_eq!(
        f.authenticate("forged").await.unwrap_err(),
        AuthError::Unauthorized
    );
    let hs = encode(
        &Header::new(Algorithm::HS256),
        &claims(),
        &EncodingKey::from_secret(b"test-client-secret"),
    )
    .unwrap();
    f.token("hs", &hs).await;
    assert_eq!(
        f.authenticate("hs").await.unwrap_err(),
        AuthError::Unauthorized
    );
    assert_eq!(
        f.requests("/keys").await,
        1,
        "invalid signatures must not trigger refreshes"
    );

    let mut c = claims();
    c["aud"] = json!(["test-client", "other"]);
    c["azp"] = json!("test-client");
    f.token("multi", &key().token(&c)).await;
    assert_eq!(f.authenticate("multi").await.unwrap(), "google-user");
}

#[tokio::test]
async fn key_rotation_refresh_is_bounded_and_coalesced() {
    let f = Fixture::new().await;
    f.keys(key()).await;
    f.token("first", &key().token(&claims())).await;
    assert!(f.authenticate("first").await.is_ok());
    f.token("unknown", &second_key().token(&claims())).await;
    assert_eq!(
        f.authenticate("unknown").await.unwrap_err(),
        AuthError::Unauthorized
    );
    assert_eq!(f.requests("/keys").await, 1);
    f.server.reset().await;
    f.keys(second_key()).await;
    f.time.store(NOW + 6, Ordering::SeqCst);
    f.token("rotate", &second_key().token(&claims())).await;
    let (a, b) = tokio::join!(f.authenticate("rotate"), f.authenticate("rotate"));
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(f.requests("/keys").await, 1);
    f.time.store(NOW + 12, Ordering::SeqCst);
    f.token("missing", &key().token(&claims())).await;
    for _ in 0..3 {
        assert_eq!(
            f.authenticate("missing").await.unwrap_err(),
            AuthError::Unauthorized
        );
    }
    assert_eq!(f.requests("/keys").await, 2);
}
#[tokio::test]
async fn expired_key_cache_fails_closed_and_recovers_after_cooldown() {
    let f = Fixture::new().await;
    f.keys(key()).await;
    f.token("initial", &key().token(&claims())).await;
    assert_eq!(f.authenticate("initial").await, Ok("google-user".into()));
    f.server.reset().await;
    // Cached keys still verify login during a key-service outage.
    f.token("cached", &key().token(&claims())).await;
    assert_eq!(f.authenticate("cached").await, Ok("google-user".into()));
    assert_eq!(f.requests("/keys").await, 0);
    f.time.store(NOW + 3600, Ordering::SeqCst);
    for code in ["expired", "cooldown"] {
        let mut c = claims();
        c["exp"] = json!(NOW + 7200);
        f.token(code, &key().token(&c)).await;
        assert_eq!(f.authenticate(code).await, Err(AuthError::Unavailable));
    }
    assert_eq!(f.requests("/keys").await, 1);
    f.time.store(NOW + 3606, Ordering::SeqCst);
    f.keys(key()).await;
    let mut c = claims();
    c["exp"] = json!(NOW + 7200);
    f.token("recovered", &key().token(&c)).await;
    assert_eq!(f.authenticate("recovered").await, Ok("google-user".into()));
    // Expiration is checked again after provider I/O.

    let f = Fixture::new().await;
    f.keys(key()).await;
    let mut c = claims();
    c["exp"] = json!(NOW + 1);
    let signed = key().token(&c);
    let time = f.time.clone();
    Mock::given(path("/token"))
        .respond_with(move |_: &wiremock::Request| {
            time.store(NOW + 1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"unused", "token_type":"Bearer", "id_token":signed}),
            )
        })
        .mount(&f.server)
        .await;
    assert_eq!(f.authenticate("slow").await, Err(AuthError::Unauthorized));
}
#[tokio::test]
async fn user_creation_and_lookup_return_persisted_models() {
    let database = Database {
        db_pool: SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap(),
    };
    database.create_tables().await.unwrap();
    assert!(matches!(
        database.get_user_by_google_sub("missing").await,
        Err(sqlx::Error::RowNotFound)
    ));
    let before = chrono::Utc::now();
    let user = database.create_user("first").await.unwrap();
    assert!(user.id > 0);
    assert_eq!(user.google_sub, "first");
    assert!(user.created_at >= before && user.created_at <= chrono::Utc::now());
    assert_eq!(
        database.get_user_by_google_sub("first").await.unwrap(),
        user
    );
    let other = database.create_user("second").await.unwrap();
    assert_ne!(user.id, other.id);
    let error = database.create_user("first").await.unwrap_err();
    assert!(matches!(error, sqlx::Error::Database(error) if error.is_unique_violation()));
    assert_eq!(
        database.get_user_by_google_sub("first").await.unwrap(),
        user
    );
}
