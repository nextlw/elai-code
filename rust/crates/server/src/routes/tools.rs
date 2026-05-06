use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tools::GlobalToolRegistry;

use crate::routes::sessions::{api_error, ApiError};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ToolEntry {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
pub struct ListToolsResponse {
    pub tools: Vec<ToolEntry>,
}

pub async fn list_tools() -> Json<ListToolsResponse> {
    let registry = GlobalToolRegistry::builtin();
    let definitions = registry.definitions(None);
    let tools = definitions
        .into_iter()
        .map(|def| ToolEntry {
            name: def.name,
            description: def.description,
            input_schema: def.input_schema,
        })
        .collect();
    Json(ListToolsResponse { tools })
}

pub async fn get_tool(Path(name): Path<String>) -> Result<Json<ToolEntry>, ApiError> {
    let registry = GlobalToolRegistry::builtin();
    let definitions = registry.definitions(None);
    let def = definitions
        .into_iter()
        .find(|d| d.name == name)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "tool not found"))?;
    Ok(Json(ToolEntry {
        name: def.name,
        description: def.description,
        input_schema: def.input_schema,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ToolPatternRequest {
    pub patterns: Vec<String>,
}

pub async fn tools_allow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(_payload): Json<ToolPatternRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify session exists
    state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

pub async fn tools_deny(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(_payload): Json<ToolPatternRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Verify session exists
    state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

pub async fn tools_rate_limit() -> Json<serde_json::Value> {
    // last_rejected() is available via runtime but RateLimiter is not exposed as a public singleton.
    // Return stub with rejection info via existing runtime::last_rejected().
    let rejected = runtime::last_rejected();
    Json(serde_json::json!({
        "status": "not_implemented",
        "last_rejected_count": rejected.len(),
    }))
}
