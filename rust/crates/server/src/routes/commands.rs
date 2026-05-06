use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::routes::sessions::{api_error, ApiError};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct CommandEntry {
    pub name: &'static str,
    pub summary: String,
    pub category: &'static str,
    pub argument_hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListCommandsResponse {
    pub commands: Vec<CommandEntry>,
}

pub async fn list_commands() -> Json<ListCommandsResponse> {
    let specs = commands::slash_command_specs();
    let commands = specs
        .iter()
        .filter(|s| !s.hidden && (s.is_enabled)())
        .map(|s| CommandEntry {
            name: s.name,
            summary: s.summary(),
            category: s.category.label(),
            argument_hint: s.argument_hint(),
        })
        .collect();
    Json(ListCommandsResponse { commands })
}

#[derive(Debug, Deserialize)]
pub struct RunCommandRequest {
    pub name: String,
    pub args: Option<String>,
}

pub async fn run_command(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<RunCommandRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    // Build full slash command string: "/name [args]"
    let full_cmd = if let Some(args) = &payload.args {
        format!("/{} {}", payload.name, args)
    } else {
        format!("/{}", payload.name)
    };

    let runtime_session = {
        let guard = session.runtime_state.lock().await;
        guard.conversation.clone()
    };

    let compaction = runtime::CompactionConfig::default();
    let result = commands::handle_slash_command(&full_cmd, &runtime_session, compaction);

    match result {
        Some(slash_result) => Ok(Json(serde_json::json!({ "output": slash_result.message }))),
        None => Ok(Json(serde_json::json!({ "status": "not_implemented", "command": full_cmd }))),
    }
}

pub async fn compact_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let runtime_session = {
        let guard = session.runtime_state.lock().await;
        guard.conversation.clone()
    };

    let compaction = runtime::CompactionConfig::default();
    let result = commands::handle_slash_command("/compact", &runtime_session, compaction);
    match result {
        Some(slash_result) => {
            // Persist compacted session back
            {
                let mut guard = session.runtime_state.lock().await;
                guard.conversation = slash_result.session;
            }
            Ok(Json(serde_json::json!({ "output": slash_result.message })))
        }
        None => Ok(Json(serde_json::json!({ "status": "not_implemented" }))),
    }
}

pub async fn export_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    Ok(Json(serde_json::json!({ "status": "not_implemented" })))
}

pub async fn resume_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}
