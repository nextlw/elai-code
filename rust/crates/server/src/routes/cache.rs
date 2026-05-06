use axum::http::StatusCode;
use axum::Json;

// ResponseCache is not exposed as a process-wide singleton in the runtime crate.
// These endpoints return stub responses accordingly.

#[derive(Debug, serde::Serialize)]
pub struct CacheStatsResponse {
    pub total_entries: usize,
    pub total_hits: u64,
    pub oldest_entry_ms: Option<u64>,
    pub newest_entry_ms: Option<u64>,
}

pub async fn cache_stats() -> Json<serde_json::Value> {
    // ResponseCache has no process-global singleton; return stub.
    Json(serde_json::json!({
        "status": "not_implemented",
        "message": "ResponseCache has no process-level singleton"
    }))
}

pub async fn cache_clear() -> StatusCode {
    // No global cache to clear; stub.
    StatusCode::OK
}
