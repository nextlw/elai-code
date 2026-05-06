use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
    Json,
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::auth::jwks::JwkSet;

/// Shared error envelope returned by all auth failures.
/// Once Task 9 promotes this to `crate::ErrorResponse`, replace the
/// definition here with `use crate::ErrorResponse;`.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Deserialize)]
struct ClerkClaims {
    sub: String,
}

/// Axum middleware that validates a Clerk-issued RS256 JWT.
///
/// Reads the bearer token from:
///   - `Authorization: Bearer <token>` header, or
///   - `?token=<token>` query parameter (for EventSource clients that
///     cannot set custom headers).
///
/// On success, inserts the resolved `db::user::User` into request
/// extensions so downstream handlers can extract it via
/// `Extension<db::user::User>`.
///
/// NOTE: `AppState.jwks` and `AppState.db` are added in Task 9.
/// This function will not compile until that task completes.
#[allow(dead_code)]
pub async fn require_auth(
    State(state): State<crate::AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let token = extract_bearer(&req)?;
    let clerk_id = verify_jwt(&token, &state.jwks).await?;

    let user = crate::db::user::get_by_clerk_id(&state.db, &clerk_id)
        .await
        .map_err(|_| internal_error("db error"))?
        .ok_or_else(|| unauthorized("user not found"))?;

    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

fn extract_bearer(req: &Request<Body>) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    // Authorization: Bearer <token>
    if let Some(header) = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = header.strip_prefix("Bearer ") {
            return Ok(token.to_owned());
        }
    }
    // ?token=<token> for EventSource (which cannot set custom headers)
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(token) = pair.strip_prefix("token=") {
                return Ok(token.to_owned());
            }
        }
    }
    Err(unauthorized("missing Authorization"))
}

async fn verify_jwt(
    token: &str,
    jwks: &Arc<RwLock<JwkSet>>,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let header = decode_header(token).map_err(|_| unauthorized("invalid JWT header"))?;
    let kid = header.kid.ok_or_else(|| unauthorized("JWT missing kid"))?;

    let jwks = jwks.read().await;
    let jwk = crate::auth::jwks::find_key(&jwks, &kid)
        .ok_or_else(|| unauthorized("unknown JWT key"))?;

    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|_| unauthorized("bad RSA key"))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = true;

    let data = decode::<ClerkClaims>(token, &decoding_key, &validation)
        .map_err(|_| unauthorized("JWT verification failed"))?;

    Ok(data.claims.sub)
}

fn unauthorized(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: msg.to_owned(),
        }),
    )
}

fn internal_error(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: msg.to_owned(),
        }),
    )
}
