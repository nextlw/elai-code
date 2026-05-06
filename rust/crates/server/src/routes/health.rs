use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub ok: bool,
    pub version: String,
    pub name: String,
}

pub async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        version: state.version.as_ref().clone(),
    })
}

pub async fn version(State(state): State<AppState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        ok: true,
        version: state.version.as_ref().clone(),
        name: "elai-server".to_string(),
    })
}
