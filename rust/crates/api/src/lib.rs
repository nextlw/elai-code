mod client;
mod error;
pub mod orchestrator;
mod providers;
mod sse;
mod types;

pub use providers::ant_models::{
    ant_default_model, get_ant_model_override_config, get_ant_models, resolve_ant_model, AntModel,
    AntModelOverrideConfig,
};
pub use providers::claude_code_spoof;

pub use client::{
    oauth_token_is_expired, read_base_url, read_xai_base_url, resolve_saved_oauth_token,
    resolve_startup_auth_source, CollectedMessageStream, MessageStream, OAuthTokenSet,
    ProviderClient,
};
pub use error::ApiError;
pub use orchestrator::{
    ElaiUnifiedAdapter, HealthReport, OpenAiUnifiedAdapter, ProviderCapability, ProviderConfig,
    ProviderOrchestrator, ProviderStatus, RequestOptions, TaskType,
};
pub use providers::elai_provider::{
    base_url_is_anthropic_official, AuthSource, ElaiApiClient, ElaiApiClient as ApiClient,
    SpoofMode,
};
pub use providers::openai_compat::{OpenAiCompatClient, OpenAiCompatConfig};
pub use providers::{
    default_thinking_config, detect_provider_kind, max_tokens_for_model,
    metadata_for_model, model_always_on_thinking, model_thinking_budget,
    model_supports_adaptive_thinking, model_supports_thinking, resolve_model_alias,
    resolve_output_config,
    suggested_default_model, ProviderKind,
};
pub use sse::{parse_frame, SseParser};
pub use types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    DocumentSource, EffortLevel, ImageSource, InputContentBlock, InputMessage, MessageDelta,
    MessageDeltaEvent, MessageRequest, MessageResponse, MessageStartEvent, MessageStopEvent,
    OutputConfig, OutputContentBlock, StreamEvent, ThinkingConfig, ToolChoice, ToolDefinition,
    ToolResultContentBlock, Usage,
};

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
