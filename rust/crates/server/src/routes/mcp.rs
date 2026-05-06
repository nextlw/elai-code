use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use runtime::{ConfigLoader, McpServerConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::routes::sessions::{api_error, ApiError};
use crate::state::AppState;

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct McpServerInfo {
    pub name: String,
    pub transport: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ListMcpServersResponse {
    pub servers: Vec<McpServerInfo>,
}

#[derive(Debug, Serialize)]
pub struct McpServerActionResponse {
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct McpToolInfo {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ListMcpToolsResponse {
    pub tools: Vec<McpToolInfo>,
}

#[derive(Debug, Serialize)]
pub struct McpResourceInfo {
    pub uri: String,
}

#[derive(Debug, Serialize)]
pub struct ListMcpResourcesResponse {
    pub resources: Vec<McpResourceInfo>,
}

#[derive(Debug, Serialize)]
pub struct McpToolCallResponse {
    pub status: String,
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct AddMcpServerRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub transport_type: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
    pub url: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct McpToolCallRequest {
    pub arguments: Option<serde_json::Value>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn transport_label(config: &McpServerConfig) -> &'static str {
    match config {
        McpServerConfig::Stdio(_) => "stdio",
        McpServerConfig::Sse(_) => "sse",
        McpServerConfig::Http(_) => "http",
        McpServerConfig::Ws(_) => "ws",
        McpServerConfig::Sdk(_) => "sdk",
        McpServerConfig::ManagedProxy(_) => "managed-proxy",
    }
}

fn load_servers() -> Vec<McpServerInfo> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loader = ConfigLoader::default_for(&cwd);
    match loader.load() {
        Ok(config) => config
            .mcp()
            .servers()
            .iter()
            .map(|(name, scoped)| McpServerInfo {
                name: name.clone(),
                transport: transport_label(&scoped.config).to_string(),
                status: "configured".to_string(),
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn list_mcp_servers(_state: State<AppState>) -> Json<ListMcpServersResponse> {
    Json(ListMcpServersResponse { servers: load_servers() })
}

pub async fn add_mcp_server(
    _state: State<AppState>,
    Json(payload): Json<AddMcpServerRequest>,
) -> Result<(StatusCode, Json<McpServerActionResponse>), ApiError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    runtime::write_project_config_json(&cwd, |root| {
        if let Some(root_obj) = root.as_object_mut() {
            root_obj
                .entry("mcpServers")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let Some(obj) = root_obj.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                let mut entry = serde_json::Map::new();
                if let Some(cmd) = &payload.command {
                    entry.insert("command".to_string(), serde_json::Value::String(cmd.clone()));
                }
                if let Some(args) = &payload.args {
                    entry.insert("args".to_string(), serde_json::to_value(args).unwrap_or_default());
                }
                if let Some(env) = &payload.env {
                    entry.insert("env".to_string(), serde_json::to_value(env).unwrap_or_default());
                }
                if let Some(url) = &payload.url {
                    entry.insert("url".to_string(), serde_json::Value::String(url.clone()));
                    if let Some(t) = &payload.transport_type {
                        entry.insert("type".to_string(), serde_json::Value::String(t.clone()));
                    }
                }
                if let Some(headers) = &payload.headers {
                    entry.insert("headers".to_string(), serde_json::to_value(headers).unwrap_or_default());
                }
                obj.insert(payload.name.clone(), serde_json::Value::Object(entry));
            }
        }
    })
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e.to_string()))?;

    Ok((StatusCode::CREATED, Json(McpServerActionResponse { status: "ok".to_string() })))
}

pub async fn update_mcp_server(
    _state: State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<McpServerActionResponse>, ApiError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let mut found = false;
    runtime::write_project_config_json(&cwd, |root| {
        if let Some(servers) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
            if let Some(entry) = servers.get_mut(&name) {
                if let (Some(entry_obj), Some(patch_obj)) = (entry.as_object_mut(), payload.as_object()) {
                    for (k, v) in patch_obj {
                        entry_obj.insert(k.clone(), v.clone());
                    }
                    found = true;
                }
            }
        }
    })
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e.to_string()))?;

    if found {
        Ok(Json(McpServerActionResponse { status: "ok".to_string() }))
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "not_found", "MCP server not found"))
    }
}

pub async fn delete_mcp_server(
    _state: State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let mut found = false;
    runtime::write_project_config_json(&cwd, |root| {
        if let Some(servers) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
            if servers.remove(&name).is_some() {
                found = true;
            }
        }
    })
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "write_failed", e.to_string()))?;

    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "not_found", "MCP server not found"))
    }
}

pub async fn restart_mcp_server(
    _state: State<AppState>,
    Path(_name): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "not_implemented",
            "message": "per-server restart not supported; McpServerManager has no restart_server API"
        })),
    )
}

pub async fn list_mcp_server_tools(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Json<ListMcpToolsResponse> {
    let guard = state.mcp.lock().await;
    let tools = guard
        .all_managed_tools()
        .filter(|t| t.server_name == name)
        .map(|t| McpToolInfo { name: t.raw_name.clone() })
        .collect();
    Json(ListMcpToolsResponse { tools })
}

pub async fn list_mcp_server_resources(
    _state: State<AppState>,
    Path(_name): Path<String>,
) -> Json<ListMcpResourcesResponse> {
    Json(ListMcpResourcesResponse { resources: Vec::new() })
}

pub async fn call_mcp_tool(
    State(state): State<AppState>,
    Path((server_name, tool)): Path<(String, String)>,
    Json(payload): Json<McpToolCallRequest>,
) -> Result<Json<McpToolCallResponse>, ApiError> {
    let qualified = runtime::mcp_tool_name(&server_name, &tool);
    let args = payload.arguments;

    let result = state
        .mcp
        .lock()
        .await
        .call_tool(&qualified, args)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "mcp_error", e.to_string()))?;

    Ok(Json(McpToolCallResponse {
        status: "ok".to_string(),
        result: Some(serde_json::to_value(result).unwrap_or(serde_json::Value::Null)),
    }))
}
