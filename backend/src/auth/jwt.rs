use super::{AuthError, Clock};
use axum::{
    body::Body,
    http::{Request, header},
    response::{IntoResponse, Response},
};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use std::future::{Ready, ready};
use tower_http::auth::AsyncAuthorizeRequest;

pub const TOKEN_SECONDS: u64 = 3600;
const ISSUER: &str = "crescend.io";
const AUDIENCE: &str = "crescend.io-api";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedUser {
    pub id: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    pub iss: String,
    pub aud: String,
}

#[derive(Clone)]
pub struct JwtAuth {
    encoding: EncodingKey,
    decoding: DecodingKey,
    clock: Clock,
}

impl JwtAuth {
    pub fn new(secret: &[u8], clock: Clock) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
            clock,
        }
    }

    pub fn issue(&self, user: i64) -> Result<String, AuthError> {
        let now = (self.clock)();
        encode(
            &Header::new(Algorithm::HS256),
            &Claims {
                sub: user.to_string(),
                iat: now,
                exp: now
                    .checked_add(TOKEN_SECONDS as i64)
                    .ok_or(AuthError::Internal)?,
                iss: ISSUER.into(),
                aud: AUDIENCE.into(),
            },
            &self.encoding,
        )
        .map_err(|_| AuthError::Internal)
    }

    pub fn verify(&self, token: &str) -> Result<AuthenticatedUser, AuthError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["sub", "iat", "exp", "iss", "aud"]);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[AUDIENCE]);
        // Apply exact time checks with the same injectable clock used by issuance.
        // Signature, algorithm, required claims, issuer and audience remain enforced by decode.
        validation.validate_exp = false;
        validation.leeway = 0;
        let claims = decode::<Claims>(token, &self.decoding, &validation)
            .map_err(|_| AuthError::Unauthorized)?
            .claims;
        let now = (self.clock)();
        if claims.exp <= now || claims.iat > now || claims.exp <= claims.iat {
            return Err(AuthError::Unauthorized);
        }
        let id = claims
            .sub
            .parse::<i64>()
            .map_err(|_| AuthError::Unauthorized)?;
        Ok(AuthenticatedUser { id })
    }
}

impl<B> AsyncAuthorizeRequest<B> for JwtAuth {
    type RequestBody = B;
    type ResponseBody = Body;
    type Future = Ready<Result<Request<B>, Response>>;

    fn authorize(&mut self, mut request: Request<B>) -> Self::Future {
        let user = (|| {
            let mut headers = request.headers().get_all(header::AUTHORIZATION).iter();
            let header = headers.next()?.to_str().ok()?;
            if headers.next().is_some() {
                return None;
            }
            let (scheme, token) = header.split_once(' ')?;
            if !scheme.eq_ignore_ascii_case("Bearer")
                || token.is_empty()
                || token.bytes().any(|c| c.is_ascii_whitespace())
            {
                return None;
            }
            self.verify(token).ok()
        })();
        ready(match user {
            Some(user) => {
                // Handlers receive the verified identity without another token or database lookup.
                request.extensions_mut().insert(user);
                Ok(request)
            }
            None => {
                let mut response = AuthError::Unauthorized.into_response();
                response.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    header::HeaderValue::from_static("Bearer"),
                );
                Err(response)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    const SECRET: &[u8] = b"test-secret-for-application-jwt-validation";

    #[test]
    fn validates_signatures_claims_and_exact_expiration() {
        let time = Arc::new(AtomicI64::new(1000));
        let clock = time.clone();
        let jwt = JwtAuth::new(SECRET, Arc::new(move || clock.load(Ordering::SeqCst)));
        let user = 42;
        let token = jwt.issue(user).unwrap();
        assert_eq!(jwt.verify(&token).unwrap().id, user);
        time.store(4599, Ordering::SeqCst);
        assert!(jwt.verify(&token).is_ok());
        time.store(4600, Ordering::SeqCst);
        assert!(jwt.verify(&token).is_err());
        time.store(1000, Ordering::SeqCst);
        let original = json!({"sub":user.to_string(),"iat":1000,"exp":4600,"iss":"crescend.io","aud":"crescend.io-api"});
        for field in ["sub", "iat", "exp", "iss", "aud"] {
            let mut claims = original.clone();
            claims.as_object_mut().unwrap().remove(field);
            let token = encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(SECRET),
            )
            .unwrap();
            assert!(jwt.verify(&token).is_err(), "accepted missing {field}");
        }
        for (field, value) in [
            ("sub", json!("google:123")),
            ("sub", json!("9223372036854775808")),
            ("sub", json!("1.5")),
            ("iss", json!("other")),
            ("aud", json!("other")),
            ("iat", json!(1001)),
            ("exp", json!(999)),
        ] {
            let mut claims = original.clone();
            claims[field] = value;
            let token = encode(
                &Header::new(Algorithm::HS256),
                &claims,
                &EncodingKey::from_secret(SECRET),
            )
            .unwrap();
            assert!(jwt.verify(&token).is_err(), "accepted invalid {field}");
        }
        for (alg, secret) in [
            (Algorithm::HS384, SECRET),
            (Algorithm::HS256, b"different-secret".as_slice()),
        ] {
            let token = encode(
                &Header::new(alg),
                &original,
                &EncodingKey::from_secret(secret),
            )
            .unwrap();
            assert!(jwt.verify(&token).is_err());
        }
        let mut tampered = token.into_bytes();
        let n = tampered.len();
        tampered[n - 5] = if tampered[n - 5] == b'A' { b'B' } else { b'A' };
        assert!(jwt.verify(std::str::from_utf8(&tampered).unwrap()).is_err());
        assert!(jwt.verify("eyJhbGciOiJub25lIn0.e30.").is_err());
    }
}

#[cfg(test)]
mod adapter_tests {
    use super::*;
    use std::sync::Arc;
    #[tokio::test]
    async fn bearer_adapter_propagates_identity_and_rejects_ambiguous_credentials() {
        let mut jwt = JwtAuth::new(
            b"test-only-signing-secret-at-least-32-bytes",
            Arc::new(|| 1000),
        );
        let token = jwt.issue(42).unwrap();
        let request = Request::builder()
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .unwrap();
        let request = jwt.authorize(request).await.unwrap();
        assert_eq!(
            request.extensions().get::<AuthenticatedUser>().unwrap().id,
            42
        );
        for values in [
            vec![],
            vec![""],
            vec!["Basic abc"],
            vec!["Bearer "],
            vec!["Bearer invalid"],
            vec!["Bearer a b"],
            vec!["Bearer invalid", "Bearer invalid"],
        ] {
            let mut request = Request::builder();
            for value in values {
                request = request.header(header::AUTHORIZATION, value);
            }
            let response = jwt.authorize(request.body(()).unwrap()).await.unwrap_err();
            assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        }
        let request = Request::builder()
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .unwrap();
        assert!(jwt.authorize(request).await.is_err());
    }
}
