use axum::extract::Path;
use axum::http::StatusCode;
use axum::Json;
use runtime::tasks::{task_registry, TaskState};
use serde::Serialize;

use crate::routes::sessions::{api_error, ApiError};

#[derive(Debug, Serialize)]
pub struct ListTasksResponse {
    pub tasks: Vec<TaskState>,
}

#[derive(Debug, Serialize)]
pub struct TaskOutputResponse {
    pub output: String,
}

pub async fn list_tasks() -> Json<ListTasksResponse> {
    let registry = task_registry();
    let tasks = registry.list_active();
    Json(ListTasksResponse { tasks })
}

pub async fn get_task(Path(id): Path<String>) -> Result<Json<TaskState>, ApiError> {
    let registry = task_registry();
    let state = registry
        .get(&id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "task not found"))?;
    Ok(Json(state))
}

pub async fn get_task_output(Path(id): Path<String>) -> Result<Json<TaskOutputResponse>, ApiError> {
    let registry = task_registry();
    let task_state = registry
        .get(&id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "task not found"))?;

    let bytes =
        runtime::tasks::task_output_path(&id);
    let output = if bytes.is_file() {
        std::fs::read_to_string(&bytes).unwrap_or_default()
    } else {
        String::new()
    };
    // suppress unused warning
    let _ = task_state.output_offset;
    Ok(Json(TaskOutputResponse { output }))
}

pub async fn cancel_task(Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let registry = task_registry();
    registry
        .kill(&id)
        .map_err(|e| api_error(StatusCode::NOT_FOUND, "not_found", e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
