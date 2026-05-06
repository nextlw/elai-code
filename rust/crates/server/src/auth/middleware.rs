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

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct ClerkClaims {
    pub sub: String,
}

pub async fn require_auth(
    State(state): State<crate::AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let token = extract_bearer(&req)?;
    let clerk_id = verify_jwt(&token, &state.jwks).await?;

    let user = match crate::db::user::get_by_clerk_id(&state.db, &clerk_id).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            // Webhook não disparou ainda (dev local sem tunnel).
            // Busca dados do usuário na Clerk API e cria o registro.
            lazy_create_user(&state.db, &clerk_id, &state.clerk_api_secret)
                .await
                .ok_or_else(|| unauthorized("user not found and could not be created"))?
        }
        Err(_) => return Err(internal_error("db error")),
    };

    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

/// Busca dados do usuário na Clerk API e cria o registro local.
/// Retorna None se a API key não estiver configurada ou a chamada falhar.
async fn lazy_create_user(
    pool: &crate::db::PgPool,
    clerk_id: &str,
    api_secret: &str,
) -> Option<crate::db::user::User> {
    if api_secret.is_empty() {
        tracing::warn!(clerk_id, "lazy user creation skipped: CLERK_SECRET_KEY not set");
        return None;
    }

    let url = format!("https://api.clerk.com/v1/users/{clerk_id}");
    let body: serde_json::Value = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {api_secret}"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let email = body["email_addresses"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|e| e["email_address"].as_str())
        .unwrap_or("")
        .to_string();

    let first = body["first_name"].as_str();
    let last  = body["last_name"].as_str();
    let name  = match (first, last) {
        (Some(f), Some(l)) => Some(format!("{f} {l}")),
        (Some(f), None)    => Some(f.to_owned()),
        (None,    Some(l)) => Some(l.to_owned()),
        _                  => None,
    };
    let avatar = body["image_url"].as_str();

    let user = crate::db::user::create(pool, clerk_id, &email, name.as_deref(), avatar)
        .await
        .ok()?;

    tracing::info!(clerk_id, email, "lazy-created user on first login");
    Some(user)
}

fn extract_bearer(req: &Request<Body>) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    // Authorization: Bearer <token> (case-insensitive scheme per RFC 7235)
    if let Some(header) = req.headers().get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some((scheme, token)) = header.split_once(' ') {
            if scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty() {
                return Ok(token.trim().to_owned());
            }
        }
    }
    // ?token=<token> for EventSource (cannot send custom headers)
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(raw) = pair.strip_prefix("token=") {
                if !raw.is_empty() {
                    // percent-decode the token value
                    let decoded = percent_decode(raw);
                    return Ok(decoded);
                }
            }
        }
    }
    Err(unauthorized("missing Authorization"))
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                result.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
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
