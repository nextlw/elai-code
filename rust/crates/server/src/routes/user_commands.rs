use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use commands::UserCommandRegistry;
use serde::{Deserialize, Serialize};
use std::io;

use crate::routes::sessions::{api_error, ApiError};
use crate::state::AppState;

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct UserCommandInfo {
    pub name: String,
    pub description: String,
    pub scope: String,
}

#[derive(Debug, Serialize)]
pub struct ListUserCommandsResponse {
    pub commands: Vec<UserCommandInfo>,
}

#[derive(Debug, Serialize)]
pub struct UserCommandActionResponse {
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserCommandRequest {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserCommandRequest {
    pub content: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn commands_dir() -> std::path::PathBuf {
    cwd().join(".elai").join("commands")
}

fn scope_label(scope: commands::UserCommandScope) -> &'static str {
    match scope {
        commands::UserCommandScope::Project => "project",
        commands::UserCommandScope::Global => "global",
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

pub async fn list_user_commands(_state: State<AppState>) -> Json<ListUserCommandsResponse> {
    let cwd = cwd();
    let registry = UserCommandRegistry::discover(&cwd).unwrap_or_default();

    let mut cmds: Vec<UserCommandInfo> = registry
        .all()
        .map(|cmd| UserCommandInfo {
            name: cmd.name.clone(),
            description: cmd.description.clone(),
            scope: scope_label(cmd.scope).to_string(),
        })
        .collect();
    cmds.sort_by(|a, b| a.name.cmp(&b.name));

    Json(ListUserCommandsResponse { commands: cmds })
}

pub async fn create_user_command(
    _state: State<AppState>,
    Json(payload): Json<CreateUserCommandRequest>,
) -> Result<(StatusCode, Json<UserCommandActionResponse>), ApiError> {
    // Validate name: must be alphanumeric + dashes/underscores
    if payload.name.is_empty()
        || !payload
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "command name must contain only alphanumeric characters, dashes, or underscores",
        ));
    }

    let dir = commands_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())
    })?;

    let file_path = dir.join(format!("{}.md", payload.name));
    std::fs::write(&file_path, &payload.content).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())
    })?;

    Ok((
        StatusCode::CREATED,
        Json(UserCommandActionResponse { status: "created".to_string() }),
    ))
}

pub async fn update_user_command(
    _state: State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<UpdateUserCommandRequest>,
) -> Result<Json<UserCommandActionResponse>, ApiError> {
    let file_path = commands_dir().join(format!("{name}.md"));
    if !file_path.exists() {
        return Err(api_error(StatusCode::NOT_FOUND, "not_found", "user command not found"));
    }

    std::fs::write(&file_path, &payload.content).map_err(|e| {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())
    })?;

    Ok(Json(UserCommandActionResponse { status: "updated".to_string() }))
}

pub async fn delete_user_command(
    _state: State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let file_path = commands_dir().join(format!("{name}.md"));
    match std::fs::remove_file(&file_path) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(api_error(StatusCode::NOT_FOUND, "not_found", "user command not found"))
        }
        Err(e) => Err(api_error(StatusCode::INTERNAL_SERVER_ERROR, "io_error", e.to_string())),
    }
}
