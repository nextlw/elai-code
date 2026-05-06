use std::process::Command;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

// ── Extended git operations ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GitCommitRequest {
    pub message: String,
    pub files: Option<Vec<String>>,
}

pub async fn git_commit(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<GitCommitRequest>,
) -> Result<Json<Value>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let cwd = &session.cwd;

    // Stage files (or all changes if none specified)
    let add_args: Vec<String> = match &payload.files {
        Some(files) if !files.is_empty() => files.clone(),
        _ => vec![".".to_string()],
    };
    let mut add_cmd_args = vec!["add".to_string()];
    add_cmd_args.extend(add_args);
    let add_refs: Vec<&str> = add_cmd_args.iter().map(String::as_str).collect();
    run_git(&add_refs, cwd)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;

    let output = run_git(&["commit", "-m", &payload.message], cwd)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;
    Ok(Json(serde_json::json!({ "output": output })))
}

#[derive(Debug, Deserialize)]
pub struct GitBranchCreateRequest {
    pub name: String,
    pub from: Option<String>,
}

pub async fn git_branch_create(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<GitBranchCreateRequest>,
) -> Result<Json<Value>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let cwd = &session.cwd;

    let output = if let Some(from) = &payload.from {
        run_git(&["checkout", "-b", &payload.name, from], cwd)
    } else {
        run_git(&["checkout", "-b", &payload.name], cwd)
    }
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;

    Ok(Json(serde_json::json!({ "output": output })))
}

#[derive(Debug, Deserialize)]
pub struct GitWorktreeCreateRequest {
    pub path: String,
    pub branch: String,
}

pub async fn git_worktree_create(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<GitWorktreeCreateRequest>,
) -> Result<Json<Value>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let cwd = &session.cwd;

    let output = run_git(&["worktree", "add", &payload.path, &payload.branch], cwd)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "git_error", e))?;
    Ok(Json(serde_json::json!({ "output": output })))
}

#[derive(Debug, Deserialize)]
pub struct GitPrCreateRequest {
    pub title: String,
    pub body: Option<String>,
    pub base: Option<String>,
}

pub async fn git_pr_create(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(payload): Json<GitPrCreateRequest>,
) -> Result<Json<Value>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let cwd = &session.cwd;

    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "-t".to_string(),
        payload.title.clone(),
    ];
    if let Some(body) = &payload.body {
        args.push("-b".to_string());
        args.push(body.clone());
    }
    if let Some(base) = &payload.base {
        args.push("-B".to_string());
        args.push(base.clone());
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = Command::new("gh")
        .args(&arg_refs)
        .current_dir(cwd)
        .output();

    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Json(serde_json::json!({ "error": "gh not installed" })))
        }
        Err(e) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "gh_error",
            e.to_string(),
        )),
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if !output.status.success() && !stderr.is_empty() {
                Ok(Json(serde_json::json!({ "error": stderr })))
            } else {
                Ok(Json(serde_json::json!({ "output": stdout })))
            }
        }
    }
}
