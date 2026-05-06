use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use runtime::{
    generate_pkce_pair, generate_state, import_claude_code_credentials, import_codex_credentials,
    load_auth_method, save_auth_method, AuthMethod, OAuthAuthorizationRequest, ApiKeyOrigin,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

use super::sessions::{api_error, ApiError};

// ── GET /v1/auth/status ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ProviderAuthStatus {
    pub provider: String,
    pub authenticated: bool,
    pub method: Option<String>,
    pub expires_at: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct AuthStatusResponse {
    pub providers: Vec<ProviderAuthStatus>,
}

pub async fn get_auth_status(_state: State<AppState>) -> Json<AuthStatusResponse> {
    let method = load_auth_method().unwrap_or(None);

    let anthropic_status = match &method {
        Some(AuthMethod::ClaudeAiOAuth { token_set, .. }) => ProviderAuthStatus {
            provider: "anthropic".to_string(),
            authenticated: true,
            method: Some("oauth".to_string()),
            expires_at: token_set.expires_at,
        },
        Some(AuthMethod::ConsoleApiKey { .. }) => ProviderAuthStatus {
            provider: "anthropic".to_string(),
            authenticated: true,
            method: Some("api_key".to_string()),
            expires_at: None,
        },
        Some(AuthMethod::AnthropicAuthToken { .. }) => ProviderAuthStatus {
            provider: "anthropic".to_string(),
            authenticated: true,
            method: Some("auth_token".to_string()),
            expires_at: None,
        },
        _ => {
            let has_env = std::env::var("ANTHROPIC_API_KEY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            ProviderAuthStatus {
                provider: "anthropic".to_string(),
                authenticated: has_env,
                method: if has_env { Some("env".to_string()) } else { None },
                expires_at: None,
            }
        }
    };

    let openai_status = {
        let has_env = std::env::var("OPENAI_API_KEY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        let oauth_status = match &method {
            Some(AuthMethod::OpenAiCodexOAuth { token_set, .. }) => Some(("oauth", token_set.expires_at)),
            Some(AuthMethod::OpenAiApiKey { .. }) => Some(("api_key", None)),
            _ => None,
        };
        if let Some((m, exp)) = oauth_status {
            ProviderAuthStatus {
                provider: "openai".to_string(),
                authenticated: true,
                method: Some(m.to_string()),
                expires_at: exp,
            }
        } else {
            ProviderAuthStatus {
                provider: "openai".to_string(),
                authenticated: has_env,
                method: if has_env { Some("env".to_string()) } else { None },
                expires_at: None,
            }
        }
    };

    let xai_status = {
        let has_env = std::env::var("XAI_API_KEY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        ProviderAuthStatus {
            provider: "xai".to_string(),
            authenticated: has_env,
            method: if has_env { Some("env".to_string()) } else { None },
            expires_at: None,
        }
    };

    Json(AuthStatusResponse {
        providers: vec![anthropic_status, openai_status, xai_status],
    })
}

// ── GET /v1/auth/methods ───────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AuthMethodInfo {
    pub provider: String,
    pub methods: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthMethodsResponse {
    pub providers: Vec<AuthMethodInfo>,
}

pub async fn list_auth_methods(_state: State<AppState>) -> Json<AuthMethodsResponse> {
    Json(AuthMethodsResponse {
        providers: vec![
            AuthMethodInfo {
                provider: "anthropic".to_string(),
                methods: vec!["api_key".to_string(), "oauth".to_string()],
            },
            AuthMethodInfo {
                provider: "openai".to_string(),
                methods: vec!["api_key".to_string()],
            },
            AuthMethodInfo {
                provider: "xai".to_string(),
                methods: vec!["api_key".to_string()],
            },
        ],
    })
}

// ── POST /v1/auth/api-key ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetApiKeyRequest {
    pub provider: String,
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct SetApiKeyResponse {
    pub status: String,
    pub provider: String,
}

pub async fn set_api_key(
    _state: State<AppState>,
    Json(payload): Json<SetApiKeyRequest>,
) -> Result<Json<SetApiKeyResponse>, ApiError> {
    match payload.provider.as_str() {
        "anthropic" => {
            let method = AuthMethod::ConsoleApiKey {
                api_key: payload.key.clone(),
                origin: ApiKeyOrigin::Pasted,
            };
            save_auth_method(&method).map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string())
            })?;
        }
        "openai" => {
            let method = AuthMethod::OpenAiApiKey { api_key: payload.key.clone() };
            save_auth_method(&method).map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string())
            })?;
        }
        "xai" => {
            // XAI uses env vars — store in secure storage via auto_store
            let store = runtime::auto_store();
            store.set("XAI_API_KEY", &payload.key).map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string())
            })?;
        }
        other => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "unknown_provider",
                format!("unknown provider: {other}"),
            ));
        }
    }

    Ok(Json(SetApiKeyResponse {
        status: "ok".to_string(),
        provider: payload.provider,
    }))
}

// ── DELETE /v1/auth/api-key/{provider} ────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DeleteApiKeyResponse {
    pub status: String,
    pub provider: String,
}

pub async fn delete_api_key(
    _state: State<AppState>,
    Path(provider): Path<String>,
) -> Result<Json<DeleteApiKeyResponse>, ApiError> {
    match provider.as_str() {
        "anthropic" | "openai" => {
            runtime::clear_auth_method().map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "delete_failed", e.to_string())
            })?;
        }
        "xai" => {
            let store = runtime::auto_store();
            store.delete("XAI_API_KEY").map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "delete_failed", e.to_string())
            })?;
        }
        other => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "unknown_provider",
                format!("unknown provider: {other}"),
            ));
        }
    }

    Ok(Json(DeleteApiKeyResponse {
        status: "ok".to_string(),
        provider,
    }))
}

// ── POST /v1/auth/oauth/start ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OAuthStartRequest {
    pub provider: String,
}

#[derive(Debug, Serialize)]
pub struct OAuthStartResponse {
    pub authorization_url: String,
    pub state: String,
}

pub async fn oauth_start(
    _state: State<AppState>,
    Json(payload): Json<OAuthStartRequest>,
) -> Result<Json<OAuthStartResponse>, ApiError> {
    match payload.provider.as_str() {
        "anthropic" => {
            let pkce = generate_pkce_pair().map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "pkce_failed", e.to_string())
            })?;
            let state_token = generate_state().map_err(|e| {
                api_error(StatusCode::INTERNAL_SERVER_ERROR, "state_failed", e.to_string())
            })?;

            let endpoints = runtime::AnthropicOAuthEndpoints::production();
            let oauth_config = endpoints.to_oauth_config(runtime::OAuthMode::ClaudeAi);

            let redirect_uri = oauth_config
                .manual_redirect_url
                .clone()
                .unwrap_or_else(|| "https://platform.claude.com/oauth/code/callback".to_string());

            let auth_request = OAuthAuthorizationRequest::from_config(
                &oauth_config,
                redirect_uri,
                &state_token,
                &pkce,
            );
            let authorization_url = auth_request.build_url();

            Ok(Json(OAuthStartResponse {
                authorization_url,
                state: state_token,
            }))
        }
        other => Err(api_error(
            StatusCode::BAD_REQUEST,
            "unknown_provider",
            format!("OAuth not supported for provider: {other}"),
        )),
    }
}

// ── GET /v1/auth/oauth/callback ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OAuthCallbackResponse {
    pub status: String,
}

pub async fn oauth_callback(
    _state: State<AppState>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Json<OAuthCallbackResponse>, ApiError> {
    if let Some(error) = query.error {
        return Err(api_error(StatusCode::BAD_REQUEST, "oauth_error", error));
    }

    let code = query.code.ok_or_else(|| {
        api_error(StatusCode::BAD_REQUEST, "missing_code", "missing code parameter")
    })?;

    // We have a code — in a full implementation we'd exchange it for a token.
    // For now we store the code as a placeholder (stub) and return ok.
    let _ = code;

    Ok(Json(OAuthCallbackResponse {
        status: "not_implemented".to_string(),
    }))
}

// ── POST /v1/auth/oauth/refresh ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OAuthRefreshRequest {
    pub provider: String,
}

#[derive(Debug, Serialize)]
pub struct OAuthRefreshResponse {
    pub status: String,
}

pub async fn oauth_refresh(
    _state: State<AppState>,
    Json(_payload): Json<OAuthRefreshRequest>,
) -> Json<OAuthRefreshResponse> {
    // Refresh flow requires HTTP client to exchange tokens — stub.
    Json(OAuthRefreshResponse {
        status: "not_implemented".to_string(),
    })
}

// ── POST /v1/auth/import/claude-code ──────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ImportResponse {
    pub status: String,
    pub imported: bool,
}

pub async fn import_claude_code(
    _state: State<AppState>,
) -> Result<Json<ImportResponse>, ApiError> {
    let result = import_claude_code_credentials().map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "import_failed", e.to_string())
    })?;

    Ok(Json(ImportResponse {
        status: "ok".to_string(),
        imported: result.is_some(),
    }))
}

// ── POST /v1/auth/import/codex ─────────────────────────────────────────────

pub async fn import_codex(
    _state: State<AppState>,
) -> Result<Json<ImportResponse>, ApiError> {
    let result = import_codex_credentials().map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "import_failed", e.to_string())
    })?;

    Ok(Json(ImportResponse {
        status: "ok".to_string(),
        imported: result.is_some(),
    }))
}
