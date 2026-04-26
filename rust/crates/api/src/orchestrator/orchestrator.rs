use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use super::metrics::ModelMetrics;
use super::provider::UnifiedProvider;
use super::types::{
    FallbackReason, OrchestrationEvent, ProviderConfig, ProviderStatus, RequestOptions,
};
use crate::error::ApiError;
use crate::types::{InputMessage, MessageRequest, MessageResponse};

const CIRCUIT_BREAKER_COOLDOWN_MS: u64 = 5 * 60 * 1000;

pub(crate) struct ProviderSlot {
    pub provider: Box<dyn UnifiedProvider>,
    pub config: ProviderConfig,
    pub status: ProviderStatus,
    pub metrics: ModelMetrics,
    pub active_concurrency: AtomicUsize,
    pub degraded_until: AtomicU64,
}

pub struct ProviderOrchestrator {
    slots: Vec<Arc<RwLock<ProviderSlot>>>,
    event_log: Arc<RwLock<VecDeque<OrchestrationEvent>>>,
    session_id: String,
}

impl std::fmt::Debug for ProviderOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderOrchestrator")
            .field("session_id", &self.session_id)
            .field("slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct HealthReport {
    pub id: String,
    pub status: ProviderStatus,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub total_calls: u64,
}

impl ProviderOrchestrator {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            event_log: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            session_id: uuid_like_id(),
        }
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn register_provider(
        &mut self,
        provider: Box<dyn UnifiedProvider>,
        config: ProviderConfig,
    ) {
        let slot = ProviderSlot {
            provider,
            config,
            status: ProviderStatus::Healthy,
            metrics: ModelMetrics::new(),
            active_concurrency: AtomicUsize::new(0),
            degraded_until: AtomicU64::new(0),
        };
        self.slots.push(Arc::new(RwLock::new(slot)));
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
        options: &RequestOptions,
    ) -> Result<MessageResponse, ApiError> {
        let candidates = self.select_candidates(options).await;
        if candidates.is_empty() {
            return Err(no_providers_error());
        }

        let selected = if options.deterministic {
            let idx = Self::deterministic_select(&request.messages, candidates.len());
            vec![candidates[idx].clone()]
        } else {
            candidates
        };

        let mut last_error: Option<ApiError> = None;
        for slot_arc in &selected {
            let start = std::time::Instant::now();

            {
                let slot = slot_arc.read().await;
                slot.active_concurrency.fetch_add(1, Ordering::Relaxed);
            }

            let result = {
                let slot = slot_arc.read().await;
                slot.provider.send_message(request).await
            };

            {
                let slot = slot_arc.read().await;
                slot.active_concurrency.fetch_sub(1, Ordering::Relaxed);
            }

            #[allow(clippy::cast_precision_loss)]
            let latency_ms = start.elapsed().as_millis() as f64;

            match result {
                Ok(response) => {
                    self.record_success(slot_arc, latency_ms, 0.0).await;
                    return Ok(response);
                }
                Err(e) => {
                    self.record_failure(slot_arc, &e.to_string()).await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(all_providers_failed_error))
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
        options: &RequestOptions,
    ) -> Result<MessageResponse, ApiError> {
        let candidates = self.select_candidates(options).await;
        if candidates.is_empty() {
            return Err(no_providers_error());
        }

        let selected = if options.deterministic {
            let idx = Self::deterministic_select(&request.messages, candidates.len());
            vec![candidates[idx].clone()]
        } else {
            candidates
        };

        let mut last_error: Option<ApiError> = None;
        for slot_arc in &selected {
            let start = std::time::Instant::now();

            {
                let slot = slot_arc.read().await;
                slot.active_concurrency.fetch_add(1, Ordering::Relaxed);
            }

            let result = {
                let slot = slot_arc.read().await;
                slot.provider.stream_message(request).await
            };

            {
                let slot = slot_arc.read().await;
                slot.active_concurrency.fetch_sub(1, Ordering::Relaxed);
            }

            #[allow(clippy::cast_precision_loss)]
            let latency_ms = start.elapsed().as_millis() as f64;

            match result {
                Ok(response) => {
                    self.record_success(slot_arc, latency_ms, 0.0).await;
                    return Ok(response);
                }
                Err(e) => {
                    self.record_failure(slot_arc, &e.to_string()).await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(all_providers_failed_error))
    }

    async fn select_candidates(
        &self,
        options: &RequestOptions,
    ) -> Vec<Arc<RwLock<ProviderSlot>>> {
        let now_ms = current_time_ms();
        let mut eligible: Vec<Arc<RwLock<ProviderSlot>>> = Vec::new();

        for slot_arc in &self.slots {
            let is_eligible = {
                let slot = slot_arc.read().await;
                if slot.config.enabled {
                    let degraded_until = slot.degraded_until.load(Ordering::Relaxed);
                    if degraded_until > 0 && now_ms < degraded_until {
                        false
                    } else {
                        if degraded_until > 0 && now_ms >= degraded_until {
                            slot.degraded_until.store(0, Ordering::Relaxed);
                        }
                        true
                    }
                } else {
                    false
                }
            };

            if is_eligible {
                eligible.push(slot_arc.clone());
            }
        }

        if options.deterministic {
            eligible
        } else {
            let mut scored: Vec<(f64, Arc<RwLock<ProviderSlot>>)> = Vec::new();
            for slot_arc in eligible {
                let score = {
                    let slot = slot_arc.read().await;
                    slot.metrics.score(options.task_type)
                };
                scored.push((score, slot_arc));
            }
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.into_iter().map(|(_, s)| s).collect()
        }
    }

    fn deterministic_select(messages: &[InputMessage], count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        let mut hasher = Sha256::new();
        for msg in messages {
            hasher.update(msg.role.as_bytes());
            hasher.update(b":");
            for block in &msg.content {
                let json = serde_json::to_string(block).unwrap_or_default();
                hasher.update(json.as_bytes());
            }
            hasher.update(b"|");
        }
        let hash = hasher.finalize();
        let index = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
        (index as usize) % count
    }

    async fn record_success(
        &self,
        slot_arc: &Arc<RwLock<ProviderSlot>>,
        latency_ms: f64,
        cost: f64,
    ) {
        let mut slot = slot_arc.write().await;
        slot.metrics.record_success(latency_ms, cost);
        slot.status = ProviderStatus::Healthy;
    }

    async fn record_failure(&self, slot_arc: &Arc<RwLock<ProviderSlot>>, error: &str) {
        let mut slot = slot_arc.write().await;
        slot.metrics.record_failure(error);
        if slot.metrics.total_calls >= 3 && slot.metrics.success_rate < 0.3 {
            let trip_until = current_time_ms() + CIRCUIT_BREAKER_COOLDOWN_MS;
            slot.degraded_until.store(trip_until, Ordering::Relaxed);
            slot.status = ProviderStatus::Degraded;
        }
    }

    pub async fn provider_health(&self) -> Vec<HealthReport> {
        let mut reports = Vec::new();
        for slot_arc in &self.slots {
            let slot = slot_arc.read().await;
            reports.push(HealthReport {
                id: slot.config.id.clone(),
                status: slot.status,
                success_rate: slot.metrics.success_rate,
                avg_latency_ms: slot.metrics.avg_latency_ms,
                total_calls: slot.metrics.total_calls,
            });
        }
        reports
    }

    pub async fn record_event(&self, event: OrchestrationEvent) {
        let mut log = self.event_log.write().await;
        if log.len() >= 1000 {
            log.pop_front();
        }
        log.push_back(event);
    }
}

impl Default for ProviderOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn extract_fallback_reason(error: &ApiError) -> FallbackReason {
    match error {
        ApiError::Api { status, .. } if status.as_u16() == 429 => FallbackReason::RateLimit,
        ApiError::Http(e) if e.is_timeout() => FallbackReason::Timeout,
        ApiError::Http(_) => FallbackReason::NetworkError,
        _ => FallbackReason::ServerError,
    }
}

fn no_providers_error() -> ApiError {
    ApiError::Api {
        status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
        error_type: Some("no_providers_available".to_string()),
        message: Some("All providers are currently unavailable".to_string()),
        body: String::new(),
        retryable: false,
    }
}

fn all_providers_failed_error() -> ApiError {
    ApiError::Api {
        status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
        error_type: Some("all_providers_failed".to_string()),
        message: Some("All providers failed".to_string()),
        body: String::new(),
        retryable: false,
    }
}

fn current_time_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("orch-{ts:x}")
}
