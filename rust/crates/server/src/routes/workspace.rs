use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use runtime::{
    edit_file, glob_search, grep_search, read_file, write_file, EditFileOutput, GlobSearchOutput,
    GrepSearchInput, GrepSearchOutput, ReadFileOutput, WriteFileOutput,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

use super::sessions::{api_error, ApiError};

// ── request / response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ReadRequest {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct EditRequest {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Deserialize)]
pub struct GlobRequest {
    pub pattern: String,
}

#[derive(Debug, Deserialize)]
pub struct GrepRequest {
    pub pattern: String,
    pub path: Option<String>,
    #[serde(default)]
    pub case_insensitive: bool,
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    pub depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct DiffQuery {
    pub staged: Option<bool>,
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiffResponse {
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub struct TreeEntry {
    pub path: String,
    pub kind: String, // "file" | "dir"
    pub depth: usize,
}

#[derive(Debug, Serialize)]
pub struct TreeResponse {
    pub cwd: String,
    pub entries: Vec<TreeEntry>,
}

// ── path-traversal guard ─────────────────────────────────────────────────────

fn resolve_and_guard(cwd: &str, user_path: &str) -> Result<PathBuf, ApiError> {
    let cwd_path = PathBuf::from(cwd);
    let cwd_canonical = cwd_path.canonicalize().map_err(|_| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "cwd_error",
            "cannot resolve session cwd",
        )
    })?;

    let candidate = if Path::new(user_path).is_absolute() {
        PathBuf::from(user_path)
    } else {
        cwd_canonical.join(user_path)
    };

    // For paths that may not exist yet (write), resolve as much as possible.
    let resolved = candidate.canonicalize().unwrap_or_else(|_| {
        // For new files, at least canonicalize the parent directory.
        if let Some(parent) = candidate.parent() {
            let canon_parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
            if let Some(name) = candidate.file_name() {
                return canon_parent.join(name);
            }
        }
        candidate.clone()
    });

    if !resolved.starts_with(&cwd_canonical) {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "path_traversal",
            "path must be within the session cwd",
        ));
    }

    Ok(resolved)
}

// ── handlers ─────────────────────────────────────────────────────────────────

pub async fn read(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(body): Json<ReadRequest>,
) -> Result<Json<ReadFileOutput>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let resolved = resolve_and_guard(&session.cwd, &body.path)?;

    tokio::task::spawn_blocking(move || {
        read_file(resolved.to_string_lossy().as_ref(), body.offset, body.limit)
    })
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "task_error", e.to_string()))?
    .map(Json)
    .map_err(|e| api_error(StatusCode::UNPROCESSABLE_ENTITY, "read_error", e.to_string()))
}

pub async fn write(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(body): Json<WriteRequest>,
) -> Result<Json<WriteFileOutput>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let resolved = resolve_and_guard(&session.cwd, &body.path)?;
    let content = body.content.clone();

    tokio::task::spawn_blocking(move || {
        write_file(resolved.to_string_lossy().as_ref(), &content)
    })
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "task_error", e.to_string()))?
    .map(Json)
    .map_err(|e| api_error(StatusCode::UNPROCESSABLE_ENTITY, "write_error", e.to_string()))
}

pub async fn edit(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(body): Json<EditRequest>,
) -> Result<Json<EditFileOutput>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let resolved = resolve_and_guard(&session.cwd, &body.path)?;
    let old_string = body.old_string.clone();
    let new_string = body.new_string.clone();
    let replace_all = body.replace_all;

    tokio::task::spawn_blocking(move || {
        edit_file(
            resolved.to_string_lossy().as_ref(),
            &old_string,
            &new_string,
            replace_all,
        )
    })
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "task_error", e.to_string()))?
    .map(Json)
    .map_err(|e| api_error(StatusCode::UNPROCESSABLE_ENTITY, "edit_error", e.to_string()))
}

pub async fn glob(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(body): Json<GlobRequest>,
) -> Result<Json<GlobSearchOutput>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let cwd = session.cwd.clone();
    let pattern = body.pattern.clone();

    tokio::task::spawn_blocking(move || glob_search(&pattern, Some(&cwd)))
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "task_error", e.to_string()))?
        .map(Json)
        .map_err(|e| api_error(StatusCode::UNPROCESSABLE_ENTITY, "glob_error", e.to_string()))
}

pub async fn grep(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(body): Json<GrepRequest>,
) -> Result<Json<GrepSearchOutput>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    // Resolve search path: either user-supplied (guarded) or the session cwd.
    let search_path = match &body.path {
        Some(p) => {
            let resolved = resolve_and_guard(&session.cwd, p)?;
            resolved.to_string_lossy().into_owned()
        }
        None => session.cwd.clone(),
    };

    let input = GrepSearchInput {
        pattern: body.pattern,
        path: Some(search_path),
        glob: None,
        output_mode: None,
        before: None,
        after: None,
        context_short: None,
        context: None,
        line_numbers: Some(true),
        case_insensitive: Some(body.case_insensitive),
        file_type: None,
        head_limit: None,
        offset: None,
        multiline: None,
    };

    tokio::task::spawn_blocking(move || grep_search(&input))
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "task_error", e.to_string()))?
        .map(Json)
        .map_err(|e| api_error(StatusCode::UNPROCESSABLE_ENTITY, "grep_error", e.to_string()))
}

pub async fn tree(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<TreeResponse>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let max_depth = query.depth.unwrap_or(3);
    let cwd = session.cwd.clone();

    let entries = tokio::task::spawn_blocking(move || collect_tree(&cwd, max_depth))
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "task_error", e.to_string()))?
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "tree_error", e))?;

    Ok(Json(TreeResponse {
        cwd: session.cwd.clone(),
        entries,
    }))
}

pub async fn diff(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<DiffResponse>, ApiError> {
    let session = state
        .sessions
        .get(&session_id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let cwd = session.cwd.clone();
    let staged = query.staged.unwrap_or(false);
    let filter_path = query.path.clone();

    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(&cwd).arg("diff");
        if staged {
            cmd.arg("--staged");
        }
        if let Some(p) = filter_path {
            cmd.arg("--").arg(p);
        }
        cmd.output()
    })
    .await
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "task_error", e.to_string()))?
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "diff_error", e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(api_error(StatusCode::UNPROCESSABLE_ENTITY, "diff_error", stderr));
    }

    Ok(Json(DiffResponse {
        diff: String::from_utf8_lossy(&output.stdout).into_owned(),
    }))
}

fn collect_tree(cwd: &str, max_depth: usize) -> Result<Vec<TreeEntry>, String> {
    let root = PathBuf::from(cwd);
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize cwd: {e}"))?;

    let mut entries = Vec::new();
    visit_dir(&canonical_root, &canonical_root, 0, max_depth, &mut entries)
        .map_err(|e| e.to_string())?;

    Ok(entries)
}

fn visit_dir(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    entries: &mut Vec<TreeEntry>,
) -> std::io::Result<()> {
    if depth > max_depth {
        return Ok(());
    }

    let mut children: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    children.sort_by_key(std::fs::DirEntry::file_name);

    for entry in children {
        let path = entry.path();
        let file_name = entry.file_name();
        // Skip hidden entries and common noisy directories.
        let name = file_name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        let kind = if path.is_dir() { "dir" } else { "file" };

        entries.push(TreeEntry {
            path: relative.to_string_lossy().into_owned(),
            kind: kind.to_string(),
            depth,
        });

        if path.is_dir() && depth < max_depth {
            visit_dir(root, &path, depth + 1, max_depth, entries)?;
        }
    }

    Ok(())
}
