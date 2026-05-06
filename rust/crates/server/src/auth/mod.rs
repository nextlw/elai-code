pub mod jwks;
pub mod middleware;

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

/// Shared bearer token verifier. Cloning is cheap (Arc).
#[derive(Clone)]
pub struct AuthState {
    pub token: Arc<String>,
}

impl AuthState {
    #[must_use]
    pub fn new(token: String) -> Self {
        Self {
            token: Arc::new(token),
        }
    }
}

pub async fn require_bearer(
    State(auth): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let provided = header
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .map(str::trim);

    match provided {
        Some(token) if token == auth.token.as_str() => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Generate a fresh random token (hex of sha256 over 32 random bytes).
#[must_use]
pub fn generate_token() -> String {
    use sha2::{Digest, Sha256};
    let mut bytes = [0u8; 32];
    for byte in &mut bytes {
        *byte = fastrand::u8(..);
    }
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
