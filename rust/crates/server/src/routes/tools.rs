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
    Json(payload): Json<ToolPatternRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let mut state = session.runtime_state.lock().await;
    for p in &payload.patterns {
        if !state.allow_patterns.contains(p) {
            state.allow_patterns.push(p.clone());
        }
    }
    let current = state.allow_patterns.clone();
    drop(state);
    Ok(Json(serde_json::json!({ "status": "ok", "allow_patterns": current })))
}

pub async fn tools_deny(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<ToolPatternRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let mut state = session.runtime_state.lock().await;
    for p in &payload.patterns {
        if !state.deny_patterns.contains(p) {
            state.deny_patterns.push(p.clone());
        }
    }
    let current = state.deny_patterns.clone();
    drop(state);
    Ok(Json(serde_json::json!({ "status": "ok", "deny_patterns": current })))
}

pub async fn tools_rate_limit() -> Json<serde_json::Value> {
    let rejected = runtime::last_rejected();
    let breakdown: Vec<serde_json::Value> = rejected
        .iter()
        .map(|r| {
            let reason = match &r.reason {
                runtime::RejectionReason::Disabled => "disabled",
                runtime::RejectionReason::SkillIncompatible(_) => "skill_incompatible",
                runtime::RejectionReason::UserFilter => "user_filter",
                runtime::RejectionReason::BudgetCap => "budget_cap",
            };
            serde_json::json!({ "tool_id": r.id, "reason": reason })
        })
        .collect();
    Json(serde_json::json!({
        "rejected_count": rejected.len(),
        "rejected": breakdown,
    }))
}
