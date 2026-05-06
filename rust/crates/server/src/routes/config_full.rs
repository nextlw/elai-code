use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use runtime::{
    load_budget_config, load_global_config, save_budget_config, save_global_config, BudgetConfig,
    ConfigLoader, GlobalConfig, ThemeOverrides,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::AppState;

use super::sessions::{api_error, ApiError};

// ── GET /v1/config ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub global: GlobalConfig,
    pub runtime: Value,
}

pub async fn get_config(_state: State<AppState>) -> Result<Json<ConfigResponse>, ApiError> {
    let global = load_global_config().map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "load_failed", e.to_string())
    })?;

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let runtime_config = ConfigLoader::default_for(&cwd).load().map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "runtime_load_failed", e.to_string())
    })?;

    let rendered = runtime_config.as_json().render();
    let runtime_value: Value = serde_json::from_str(&rendered)
        .unwrap_or(Value::Object(serde_json::Map::new()));

    Ok(Json(ConfigResponse { global, runtime: runtime_value }))
}

// ── PATCH /v1/config ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PatchConfigRequest {
    pub setup_complete: Option<bool>,
    pub default_model: Option<String>,
    pub default_permission_mode: Option<String>,
    pub locale: Option<String>,
}

pub async fn patch_config(
    _state: State<AppState>,
    Json(payload): Json<PatchConfigRequest>,
) -> Result<Json<GlobalConfig>, ApiError> {
    let mut config = load_global_config().map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "load_failed", e.to_string())
    })?;

    if let Some(v) = payload.setup_complete {
        config.setup_complete = v;
    }
    if let Some(v) = payload.default_model {
        config.default_model = v;
    }
    if let Some(v) = payload.default_permission_mode {
        config.default_permission_mode = v;
    }
    if let Some(v) = payload.locale {
        config.locale = v;
    }

    save_global_config(&config).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string())
    })?;

    Ok(Json(config))
}

// ── GET /v1/config/sources ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ConfigSource {
    pub source: String,
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Serialize)]
pub struct ConfigSourcesResponse {
    pub sources: Vec<ConfigSource>,
}

pub async fn get_config_sources(_state: State<AppState>) -> Json<ConfigSourcesResponse> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loader = ConfigLoader::default_for(&cwd);
    let entries = loader.discover();

    let sources = entries
        .into_iter()
        .map(|entry| {
            let path_str = entry.path.display().to_string();
            let exists = entry.path.is_file();
            let source_label = format!("{:?}", entry.source).to_lowercase();
            ConfigSource {
                source: source_label,
                path: path_str,
                exists,
            }
        })
        .collect();

    Json(ConfigSourcesResponse { sources })
}

// ── POST /v1/providers/{id}/test ───────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ProviderTestResponse {
    pub provider: String,
    pub status: String,
    pub message: String,
}

pub async fn test_provider(
    _state: State<AppState>,
    Path(id): Path<String>,
) -> Json<ProviderTestResponse> {
    let (env_var, label) = match id.as_str() {
        "anthropic" => ("ANTHROPIC_API_KEY", "Anthropic"),
        "openai" => ("OPENAI_API_KEY", "OpenAI"),
        "xai" => ("XAI_API_KEY", "xAI"),
        "ollama" => ("OLLAMA_BASE_URL", "Ollama"),
        "lmstudio" => ("LMSTUDIO_BASE_URL", "LM Studio"),
        other => {
            return Json(ProviderTestResponse {
                provider: other.to_string(),
                status: "error".to_string(),
                message: format!("unknown provider: {other}"),
            });
        }
    };

    let has_key = std::env::var(env_var)
        .is_ok_and(|v| !v.trim().is_empty());

    if has_key {
        Json(ProviderTestResponse {
            provider: id,
            status: "ok".to_string(),
            message: format!("{label} key found in environment"),
        })
    } else {
        // Also check stored auth
        let auth_ok = runtime::load_auth_method()
            .unwrap_or(None)
            .is_some_and(|m| matches!(
                (&id[..], m),
                ("anthropic",
runtime::AuthMethod::ConsoleApiKey { .. } |
runtime::AuthMethod::ClaudeAiOAuth { .. } |
runtime::AuthMethod::AnthropicAuthToken { .. }) |
("openai",
runtime::AuthMethod::OpenAiApiKey { .. } |
runtime::AuthMethod::OpenAiCodexOAuth { .. })
            ));

        if auth_ok {
            Json(ProviderTestResponse {
                provider: id,
                status: "ok".to_string(),
                message: format!("{label} credentials found in credential store"),
            })
        } else {
            Json(ProviderTestResponse {
                provider: id,
                status: "error".to_string(),
                message: format!("{label} credentials not configured"),
            })
        }
    }
}

// ── GET /v1/budget ─────────────────────────────────────────────────────────

pub async fn get_budget(_state: State<AppState>) -> Json<BudgetConfig> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let config = load_budget_config(&cwd).unwrap_or_default();
    Json(config)
}

// ── PATCH /v1/budget ───────────────────────────────────────────────────────

pub async fn patch_budget(
    _state: State<AppState>,
    Json(payload): Json<BudgetConfig>,
) -> Result<Json<BudgetConfig>, ApiError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    save_budget_config(&cwd, &payload).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string())
    })?;
    Ok(Json(payload))
}

// ── GET /v1/theme ──────────────────────────────────────────────────────────

pub async fn get_theme(_state: State<AppState>) -> Result<Json<ThemeOverrides>, ApiError> {
    let config = load_global_config().map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "load_failed", e.to_string())
    })?;
    Ok(Json(config.theme))
}

// ── PATCH /v1/theme ────────────────────────────────────────────────────────

pub async fn patch_theme(
    _state: State<AppState>,
    Json(payload): Json<ThemeOverrides>,
) -> Result<Json<ThemeOverrides>, ApiError> {
    let mut config = load_global_config().map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "load_failed", e.to_string())
    })?;

    config.theme = payload.clone();

    save_global_config(&config).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", e.to_string())
    })?;

    Ok(Json(payload))
}
