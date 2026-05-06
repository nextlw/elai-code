use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct CacheStatsResponse {
    pub total_entries: usize,
    pub total_hits: u64,
    pub oldest_entry_ms: Option<u64>,
    pub newest_entry_ms: Option<u64>,
}

pub async fn cache_stats(State(state): State<AppState>) -> Json<CacheStatsResponse> {
    let guard = state.response_cache.lock().await;
    let s = guard.stats();
    Json(CacheStatsResponse {
        total_entries: s.total_entries,
        total_hits: s.total_hits,
        oldest_entry_ms: s.oldest_entry_ms,
        newest_entry_ms: s.newest_entry_ms,
    })
}

pub async fn cache_clear(State(state): State<AppState>) -> StatusCode {
    state.response_cache.lock().await.clear();
    StatusCode::NO_CONTENT
}
