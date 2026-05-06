use axum::extract::Query;
use axum::Json;
use runtime::{default_telemetry_path, load_entries, TelemetryEntry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct TelemetryQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct TelemetryResponse {
    pub entries: Vec<TelemetryEntry>,
}

pub async fn get_telemetry(Query(params): Query<TelemetryQuery>) -> Json<TelemetryResponse> {
    let path = default_telemetry_path();
    let mut entries = load_entries(&path, None).unwrap_or_default();
    if let Some(limit) = params.limit {
        let start = entries.len().saturating_sub(limit);
        entries = entries.into_iter().skip(start).collect();
    }
    Json(TelemetryResponse { entries })
}

#[derive(Debug, Serialize)]
pub struct UsageSummaryResponse {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_write_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cost_usd: f64,
    pub request_count: usize,
}

pub async fn get_usage_summary() -> Json<UsageSummaryResponse> {
    let path = default_telemetry_path();
    let entries = load_entries(&path, None).unwrap_or_default();

    let request_count = entries.len();
    let total_input_tokens: u64 = entries.iter().map(|e| e.input_tokens).sum();
    let total_output_tokens: u64 = entries.iter().map(|e| e.output_tokens).sum();
    let total_cache_write_tokens: u64 = entries.iter().map(|e| e.cache_write_tokens).sum();
    let total_cache_read_tokens: u64 = entries.iter().map(|e| e.cache_read_tokens).sum();
    let total_cost_usd: f64 = entries.iter().map(|e| e.cost_usd).sum();

    Json(UsageSummaryResponse {
        total_input_tokens,
        total_output_tokens,
        total_cache_write_tokens,
        total_cache_read_tokens,
        total_cost_usd,
        request_count,
    })
}
