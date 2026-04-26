use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::metrics::ModelMetrics;

#[derive(Serialize, Deserialize, Default)]
pub struct PersistedMetrics {
    pub version: u32,
    pub providers: Vec<PersistedProviderMetrics>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PersistedProviderMetrics {
    pub id: String,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub total_calls: u64,
    pub total_failures: u64,
}

#[must_use]
pub fn metrics_path() -> PathBuf {
    let config_dir = std::env::var("ELAI_CONFIG_HOME").map_or_else(
        |_| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("claw")
        },
        PathBuf::from,
    );
    config_dir.join("orchestrator-metrics.json")
}

pub fn save_metrics(metrics: &[(&str, &ModelMetrics)]) -> Result<(), std::io::Error> {
    let path = metrics_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = PersistedMetrics {
        version: 1,
        providers: metrics
            .iter()
            .map(|(id, m)| PersistedProviderMetrics {
                id: (*id).to_string(),
                success_rate: m.success_rate,
                avg_latency_ms: m.avg_latency_ms,
                total_calls: m.total_calls,
                total_failures: m.total_failures,
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&data)
        .map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(())
}

pub fn load_metrics() -> Result<Option<PersistedMetrics>, std::io::Error> {
    let path = metrics_path();
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path)?;
    let data: PersistedMetrics = serde_json::from_str(&json)
        .map_err(std::io::Error::other)?;
    Ok(Some(data))
}
