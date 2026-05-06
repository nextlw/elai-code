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
                .is_ok_and(|v| !v.trim().is_empty());
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
            .is_ok_and(|v| !v.trim().is_empty());
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
            .is_ok_and(|v| !v.trim().is_empty());
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
    State(state): State<AppState>,
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
                redirect_uri.clone(),
                &state_token,
                &pkce,
            );
            let authorization_url = auth_request.build_url();

            // Persist pending OAuth state for callback validation
            {
                let created_at = crate::session_store::now_millis();
                let pending = crate::state::OAuthPendingState {
                    pkce,
                    redirect_uri,
                    provider: "anthropic".to_string(),
                    created_at,
                };
                state.oauth_pending.lock().await.insert(state_token.clone(), pending);
            }

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
    State(state): State<AppState>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Json<OAuthCallbackResponse>, ApiError> {
    if let Some(error) = query.error {
        return Err(api_error(StatusCode::BAD_REQUEST, "oauth_error", error));
    }

    let code = query.code.ok_or_else(|| {
        api_error(StatusCode::BAD_REQUEST, "missing_code", "missing code parameter")
    })?;
    let state_token = query.state.ok_or_else(|| {
        api_error(StatusCode::BAD_REQUEST, "missing_state", "missing state parameter")
    })?;

    // Recover pending PKCE state
    let pending = state.oauth_pending.lock().await.remove(&state_token)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_state", "unknown or expired OAuth state"))?;

    // Build OAuth config for the provider
    let oauth_config = match pending.provider.as_str() {
        "anthropic" => runtime::AnthropicOAuthEndpoints::production()
            .to_oauth_config(runtime::OAuthMode::ClaudeAi),
        other => return Err(api_error(StatusCode::BAD_REQUEST, "unknown_provider",
            format!("OAuth not supported for provider: {other}"))),
    };

    // Exchange code for token
    let exchange = runtime::OAuthTokenExchangeRequest::from_config(
        &oauth_config,
        code,
        state_token,
        pending.pkce.verifier.clone(),
        pending.redirect_uri.clone(),
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&oauth_config.token_url)
        .form(&exchange.form_params())
        .send()
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, "token_exchange_failed", e.to_string()))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(api_error(StatusCode::BAD_GATEWAY, "token_exchange_error", body));
    }

    let token_data: serde_json::Value = resp.json().await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, "token_parse_failed", e.to_string()))?;

    let access_token = token_data["access_token"].as_str()
        .ok_or_else(|| api_error(StatusCode::BAD_GATEWAY, "missing_access_token", "no access_token in response"))?
        .to_string();
    let refresh_token = token_data["refresh_token"].as_str().map(str::to_string);
    let expires_in = token_data["expires_in"].as_u64();
    let expires_at = expires_in.map(|secs| crate::session_store::now_millis() / 1000 + secs);

    let token_set = runtime::OAuthTokenSet {
        access_token,
        refresh_token,
        expires_at,
        scopes: oauth_config.scopes.clone(),
    };

    runtime::save_oauth_credentials(&token_set)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string()))?;

    let auth_method = runtime::AuthMethod::ClaudeAiOAuth {
        token_set,
        subscription: None,
    };
    runtime::save_auth_method(&auth_method)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_auth_failed", e.to_string()))?;

    Ok(Json(OAuthCallbackResponse { status: "ok".to_string() }))
}

// ── POST /v1/auth/oauth/refresh ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OAuthRefreshPayload {
    pub provider: String,
}

#[derive(Debug, Serialize)]
pub struct OAuthRefreshResponse {
    pub status: String,
}

pub async fn oauth_refresh(
    _state: State<AppState>,
    Json(payload): Json<OAuthRefreshPayload>,
) -> Result<Json<OAuthRefreshResponse>, ApiError> {
    let auth = runtime::load_auth_method()
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "load_failed", e.to_string()))?;

    let (token_set, oauth_config) = match (&auth, payload.provider.as_str()) {
        (Some(runtime::AuthMethod::ClaudeAiOAuth { token_set, .. }), "anthropic") => {
            let cfg = runtime::AnthropicOAuthEndpoints::production()
                .to_oauth_config(runtime::OAuthMode::ClaudeAi);
            (token_set.clone(), cfg)
        }
        _ => return Err(api_error(StatusCode::BAD_REQUEST, "no_refresh_token",
            "no OAuth credentials found for provider")),
    };

    let refresh_token = token_set.refresh_token.ok_or_else(|| {
        api_error(StatusCode::BAD_REQUEST, "no_refresh_token", "no refresh token available")
    })?;

    let refresh_req = runtime::OAuthRefreshRequest::from_config(&oauth_config, refresh_token, None);

    let client = reqwest::Client::new();
    let resp = client
        .post(&oauth_config.token_url)
        .form(&refresh_req.form_params())
        .send()
        .await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, "refresh_failed", e.to_string()))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(api_error(StatusCode::BAD_GATEWAY, "refresh_error", body));
    }

    let token_data: serde_json::Value = resp.json().await
        .map_err(|e| api_error(StatusCode::BAD_GATEWAY, "token_parse_failed", e.to_string()))?;

    let access_token = token_data["access_token"].as_str()
        .ok_or_else(|| api_error(StatusCode::BAD_GATEWAY, "missing_access_token", "no access_token in response"))?
        .to_string();
    let new_refresh = token_data["refresh_token"].as_str().map(str::to_string);
    let expires_in = token_data["expires_in"].as_u64();
    let expires_at = expires_in.map(|secs| crate::session_store::now_millis() / 1000 + secs);

    let new_token_set = runtime::OAuthTokenSet {
        access_token,
        refresh_token: new_refresh,
        expires_at,
        scopes: oauth_config.scopes.clone(),
    };

    runtime::save_oauth_credentials(&new_token_set)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string()))?;

    Ok(Json(OAuthRefreshResponse { status: "ok".to_string() }))
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
