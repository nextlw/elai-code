use std::process::Command;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::routes::sessions::{api_error, ApiError};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct GitOutputResponse {
    pub output: String,
}

#[derive(Debug, Deserialize)]
pub struct GitLogQuery {
    pub limit: Option<usize>,
}

fn run_git(args: &[&str], cwd: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() && !stderr.is_empty() {
        return Err(stderr);
    }
    Ok(stdout)
}

pub async fn git_status(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<GitOutputResponse>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let output = run_git(&["status", "--porcelain"], &session.cwd)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;
    Ok(Json(GitOutputResponse { output }))
}

pub async fn git_diff(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<GitOutputResponse>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let output = run_git(&["diff"], &session.cwd)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;
    Ok(Json(GitOutputResponse { output }))
}

pub async fn git_log(
    Path(session_id): Path<String>,
    Query(params): Query<GitLogQuery>,
    State(state): State<AppState>,
) -> Result<Json<GitOutputResponse>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let limit = params.limit.unwrap_or(10).to_string();
    let output = run_git(
        &["log", "--oneline", &format!("-{limit}")],
        &session.cwd,
    )
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;
    Ok(Json(GitOutputResponse { output }))
}

pub async fn git_branches(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<GitOutputResponse>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let output = run_git(&["branch", "-a"], &session.cwd)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;
    Ok(Json(GitOutputResponse { output }))
}
