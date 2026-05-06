use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use plugins::{PluginError, PluginManager, PluginManagerConfig};
use runtime::{load_all_skills, validate_skills, ConfigLoader};
use serde::Serialize;

use crate::routes::sessions::{api_error, ApiError};
use crate::state::AppState;

struct NoopReporter;
impl code_index::ProgressReporter for NoopReporter {
    fn report(&self, _msg: &str) {}
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub kind: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct ListPluginsResponse {
    pub plugins: Vec<PluginInfo>,
}

#[derive(Debug, Serialize)]
pub struct PluginActionResponse {
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub priority: i32,
}

#[derive(Debug, Serialize)]
pub struct ListSkillsResponse {
    pub skills: Vec<SkillInfo>,
}

#[derive(Debug, Serialize)]
pub struct SkillValidationResponse {
    pub valid: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentInfo {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentInfo>,
}

#[derive(Debug, Serialize)]
pub struct AgentRunResponse {
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct HookInfo {
    pub event: String,
    pub commands: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ListHooksResponse {
    pub hooks: Vec<HookInfo>,
}

#[derive(Debug, Serialize)]
pub struct HookUpdateResponse {
    pub status: String,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

fn resolve_config_home() -> std::path::PathBuf {
    std::env::var_os("ELAI_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".elai"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".elai"))
}

pub async fn list_plugins(_state: State<AppState>) -> Json<ListPluginsResponse> {
    let config_home = resolve_config_home();
    let manager_config = PluginManagerConfig::new(config_home);
    let manager = PluginManager::new(manager_config);

    let plugins = match manager.list_plugins() {
        Ok(summaries) => summaries
            .into_iter()
            .map(|s| PluginInfo {
                id: s.metadata.id.clone(),
                name: s.metadata.name.clone(),
                version: s.metadata.version.clone(),
                description: s.metadata.description.clone(),
                kind: s.metadata.kind.to_string(),
                enabled: s.enabled,
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    Json(ListPluginsResponse { plugins })
}

pub async fn install_plugin(
    _state: State<AppState>,
    Path(name): Path<String>,
) -> Result<(StatusCode, Json<PluginActionResponse>), ApiError> {
    let config_home = resolve_config_home();
    let manager_config = PluginManagerConfig::new(config_home);
    let mut manager = PluginManager::new(manager_config);
    let reporter = NoopReporter;
    manager.install(&name, &reporter).map_err(|e| match e {
        PluginError::NotFound(_) => api_error(StatusCode::NOT_FOUND, "not_found", e.to_string()),
        _ => api_error(StatusCode::INTERNAL_SERVER_ERROR, "install_failed", e.to_string()),
    })?;
    Ok((StatusCode::CREATED, Json(PluginActionResponse { status: "ok".to_string() })))
}

pub async fn update_plugin(
    _state: State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<PluginActionResponse>, ApiError> {
    let config_home = resolve_config_home();
    let manager_config = PluginManagerConfig::new(config_home);
    let mut manager = PluginManager::new(manager_config);
    let reporter = NoopReporter;
    manager.update(&name, &reporter).map_err(|e| match e {
        PluginError::NotFound(_) => api_error(StatusCode::NOT_FOUND, "not_found", e.to_string()),
        _ => api_error(StatusCode::INTERNAL_SERVER_ERROR, "update_failed", e.to_string()),
    })?;
    Ok(Json(PluginActionResponse { status: "ok".to_string() }))
}

pub async fn uninstall_plugin(
    _state: State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let config_home = resolve_config_home();
    let manager_config = PluginManagerConfig::new(config_home);
    let mut manager = PluginManager::new(manager_config);
    manager.uninstall(&name).map_err(|e| match e {
        PluginError::NotFound(_) => api_error(StatusCode::NOT_FOUND, "not_found", e.to_string()),
        _ => api_error(StatusCode::INTERNAL_SERVER_ERROR, "uninstall_failed", e.to_string()),
    })?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_skills(_state: State<AppState>) -> Json<ListSkillsResponse> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let skills = load_all_skills(&cwd);
    Json(ListSkillsResponse {
        skills: skills
            .into_iter()
            .map(|s| SkillInfo {
                name: s.metadata.name.clone(),
                description: s.metadata.description.clone(),
                version: s.metadata.version.clone(),
                priority: s.metadata.priority,
            })
            .collect(),
    })
}

pub async fn validate_skills_route(_state: State<AppState>) -> Json<SkillValidationResponse> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let skills = load_all_skills(&cwd);
    let result = validate_skills(&skills);
    Json(SkillValidationResponse {
        valid: result.valid,
        errors: result.errors,
    })
}

pub async fn list_agents(_state: State<AppState>) -> Json<ListAgentsResponse> {
    let config_home = resolve_config_home();
    let manager_config = PluginManagerConfig::new(config_home);
    let manager = PluginManager::new(manager_config);

    let agents = match manager.list_plugins() {
        Ok(summaries) => summaries
            .into_iter()
            .filter(|s| s.metadata.kind.to_string().to_lowercase().contains("agent"))
            .map(|s| AgentInfo { name: s.metadata.name.clone() })
            .collect(),
        Err(_) => Vec::new(),
    };

    Json(ListAgentsResponse { agents })
}

pub async fn run_agent(
    _state: State<AppState>,
    Path(_name): Path<String>,
    _body: Option<Json<serde_json::Value>>,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "not_implemented",
            "message": "agent execution pipeline not wired in server runtime"
        })),
    )
}

pub async fn list_hooks(_state: State<AppState>) -> Json<ListHooksResponse> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loader = ConfigLoader::default_for(&cwd);
    let hooks = match loader.load() {
        Ok(config) => {
            let hook_config = config.hooks();
            vec![
                HookInfo {
                    event: "PreToolUse".to_string(),
                    commands: hook_config.pre_tool_use().to_vec(),
                },
                HookInfo {
                    event: "PostToolUse".to_string(),
                    commands: hook_config.post_tool_use().to_vec(),
                },
            ]
        }
        Err(_) => Vec::new(),
    };

    Json(ListHooksResponse { hooks })
}

pub async fn update_hooks(
    _state: State<AppState>,
    _body: Option<Json<serde_json::Value>>,
) -> Json<HookUpdateResponse> {
    Json(HookUpdateResponse { status: "not_implemented".to_string() })
}
