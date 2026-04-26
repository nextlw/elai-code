#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderCapability {
    Thinking,
    ToolCalling,
    Streaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatus {
    Healthy,
    Degraded,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    Chat,
    Code,
    Analysis,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    RateLimit,
    Timeout,
    ServerError,
    NetworkError,
    CapabilityMismatch,
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: String,
    pub priority: usize,
    pub enabled: bool,
    pub max_concurrency: usize,
}

#[derive(Debug, Clone)]
pub struct OrchestrationEvent {
    pub timestamp: std::time::Instant,
    pub session_id: String,
    pub command: String,
    pub primary_provider: String,
    pub actual_provider: String,
    pub fallback_reason: Option<FallbackReason>,
    pub latency_ms: u64,
    pub retry_count: u32,
}

#[derive(Debug, Clone)]
pub struct RequestOptions {
    pub task_type: TaskType,
    pub deterministic: bool,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            task_type: TaskType::Unknown,
            deterministic: false,
        }
    }
}
