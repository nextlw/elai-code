pub mod metrics;
#[allow(clippy::module_inception)]
pub mod orchestrator;
pub mod persistence;
pub mod provider;
pub mod types;

pub use metrics::ModelMetrics;
pub use orchestrator::{extract_fallback_reason, HealthReport, ProviderOrchestrator};
pub use provider::{ElaiUnifiedAdapter, OpenAiUnifiedAdapter, UnifiedProvider};
pub use types::{
    FallbackReason, OrchestrationEvent, ProviderCapability, ProviderConfig, ProviderStatus,
    RequestOptions, TaskType,
};
