use super::types::TaskType;

pub const EMA_ALPHA: f64 = 0.3;

#[derive(Debug, Clone)]
pub struct ModelMetrics {
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub prev_success_rate: f64,
    pub prev_avg_latency_ms: f64,
    pub cost_per_1k: f64,
    pub total_calls: u64,
    pub total_failures: u64,
    pub last_error: Option<String>,
    pub last_error_time: Option<std::time::Instant>,
}

impl ModelMetrics {
    #[must_use]
    pub fn new() -> Self {
        Self {
            success_rate: 1.0,
            avg_latency_ms: 1000.0,
            prev_success_rate: 1.0,
            prev_avg_latency_ms: 1000.0,
            cost_per_1k: 0.0,
            total_calls: 0,
            total_failures: 0,
            last_error: None,
            last_error_time: None,
        }
    }

    pub fn record_success(&mut self, latency_ms: f64, cost: f64) {
        self.prev_success_rate = self.success_rate;
        self.prev_avg_latency_ms = self.avg_latency_ms;
        self.success_rate = self.success_rate.mul_add(1.0 - EMA_ALPHA, EMA_ALPHA);
        self.avg_latency_ms = self
            .avg_latency_ms
            .mul_add(1.0 - EMA_ALPHA, latency_ms * EMA_ALPHA);
        self.cost_per_1k = self.cost_per_1k.mul_add(1.0 - EMA_ALPHA, cost * EMA_ALPHA);
        self.total_calls += 1;
    }

    pub fn record_failure(&mut self, error: &str) {
        self.prev_success_rate = self.success_rate;
        self.prev_avg_latency_ms = self.avg_latency_ms;
        self.success_rate *= 1.0 - EMA_ALPHA;
        self.total_calls += 1;
        self.total_failures += 1;
        self.last_error = Some(error.to_string());
        self.last_error_time = Some(std::time::Instant::now());
    }

    #[must_use]
    pub fn score(&self, _task_type: TaskType) -> f64 {
        let latency_penalty = 1.0 / (1.0 + self.avg_latency_ms / 1000.0);
        self.success_rate.mul_add(0.7, latency_penalty * 0.3)
    }
}

impl Default for ModelMetrics {
    fn default() -> Self {
        Self::new()
    }
}
