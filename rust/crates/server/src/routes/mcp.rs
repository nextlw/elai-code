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
    Json(_payload): Json<AddMcpServerRequest>,
) -> (StatusCode, Json<McpServerActionResponse>) {
    (
        StatusCode::CREATED,
        Json(McpServerActionResponse { status: "not_implemented".to_string() }),
    )
}

pub async fn update_mcp_server(
    _state: State<AppState>,
    Path(_name): Path<String>,
    Json(_payload): Json<serde_json::Value>,
) -> Json<McpServerActionResponse> {
    Json(McpServerActionResponse { status: "not_implemented".to_string() })
}

pub async fn delete_mcp_server(
    _state: State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let servers = load_servers();
    if servers.iter().any(|s| s.name == name) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "not_found", "MCP server not found"))
    }
}

pub async fn restart_mcp_server(
    _state: State<AppState>,
    Path(_name): Path<String>,
) -> Json<McpServerActionResponse> {
    Json(McpServerActionResponse { status: "restarted".to_string() })
}

pub async fn list_mcp_server_tools(
    _state: State<AppState>,
    Path(_name): Path<String>,
) -> Json<ListMcpToolsResponse> {
    Json(ListMcpToolsResponse { tools: Vec::new() })
}

pub async fn list_mcp_server_resources(
    _state: State<AppState>,
    Path(_name): Path<String>,
) -> Json<ListMcpResourcesResponse> {
    Json(ListMcpResourcesResponse { resources: Vec::new() })
}

pub async fn call_mcp_tool(
    _state: State<AppState>,
    Path((_name, _tool)): Path<(String, String)>,
    Json(_payload): Json<McpToolCallRequest>,
) -> Json<McpToolCallResponse> {
    Json(McpToolCallResponse {
        status: "not_implemented".to_string(),
        result: None,
    })
}
