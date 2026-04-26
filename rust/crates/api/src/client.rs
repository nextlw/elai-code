use std::sync::Arc;

use crate::error::ApiError;
use crate::orchestrator::{
    ElaiUnifiedAdapter, OpenAiUnifiedAdapter, ProviderConfig, ProviderOrchestrator, RequestOptions,
};
use crate::providers::elai_provider::{self, AuthSource, ElaiApiClient};
use crate::providers::openai_compat::{self, OpenAiCompatClient, OpenAiCompatConfig};
use crate::providers::{self, Provider, ProviderKind};
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    MessageDelta, MessageDeltaEvent, MessageRequest, MessageResponse, MessageStartEvent,
    MessageStopEvent, OutputContentBlock, StreamEvent,
};

async fn send_via_provider<P: Provider>(
    provider: &P,
    request: &MessageRequest,
) -> Result<MessageResponse, ApiError> {
    provider.send_message(request).await
}

async fn stream_via_provider<P: Provider>(
    provider: &P,
    request: &MessageRequest,
) -> Result<P::Stream, ApiError> {
    provider.stream_message(request).await
}

#[derive(Debug, Clone)]
pub enum ProviderClient {
    ElaiApi(ElaiApiClient),
    Xai(OpenAiCompatClient),
    OpenAi(OpenAiCompatClient),
    Orchestrated(Arc<ProviderOrchestrator>),
}

impl ProviderClient {
    pub fn from_model(model: &str) -> Result<Self, ApiError> {
        Self::from_model_with_default_auth(model, None)
    }

    pub fn from_model_with_default_auth(
        model: &str,
        default_auth: Option<AuthSource>,
    ) -> Result<Self, ApiError> {
        let resolved_model = providers::resolve_model_alias(model);
        match providers::detect_provider_kind(&resolved_model) {
            ProviderKind::ElaiApi => Ok(Self::ElaiApi(match default_auth {
                Some(auth) => ElaiApiClient::from_auth(auth),
                None => ElaiApiClient::from_env()?,
            })),
            ProviderKind::Xai => Ok(Self::Xai(OpenAiCompatClient::from_env(
                OpenAiCompatConfig::xai(),
            )?)),
            ProviderKind::OpenAi => Ok(Self::OpenAi(OpenAiCompatClient::from_env(
                OpenAiCompatConfig::openai(),
            )?)),
        }
    }

    /// Build an orchestrated `ProviderClient` that registers every provider with
    /// available credentials and falls back across them.
    pub fn orchestrated() -> Result<Self, ApiError> {
        let mut orchestrator = ProviderOrchestrator::new();
        let mut priority = 0_usize;
        let mut registered_any = false;

        if elai_provider::has_auth_from_env_or_saved().unwrap_or(false) {
            if let Ok(client) = ElaiApiClient::from_env() {
                orchestrator.register_provider(
                    Box::new(ElaiUnifiedAdapter::new(client)),
                    ProviderConfig {
                        id: "anthropic".to_string(),
                        priority,
                        enabled: true,
                        max_concurrency: 4,
                    },
                );
                priority += 1;
                registered_any = true;
            }
        }

        if openai_compat::has_api_key("OPENAI_API_KEY") {
            if let Ok(client) = OpenAiCompatClient::from_env(OpenAiCompatConfig::openai()) {
                orchestrator.register_provider(
                    Box::new(OpenAiUnifiedAdapter::new(client, "openai")),
                    ProviderConfig {
                        id: "openai".to_string(),
                        priority,
                        enabled: true,
                        max_concurrency: 4,
                    },
                );
                priority += 1;
                registered_any = true;
            }
        }

        if openai_compat::has_api_key("XAI_API_KEY") {
            if let Ok(client) = OpenAiCompatClient::from_env(OpenAiCompatConfig::xai()) {
                orchestrator.register_provider(
                    Box::new(OpenAiUnifiedAdapter::new(client, "xai")),
                    ProviderConfig {
                        id: "xai".to_string(),
                        priority,
                        enabled: true,
                        max_concurrency: 4,
                    },
                );
                registered_any = true;
            }
        }

        if !registered_any {
            return Err(ApiError::missing_credentials(
                "orchestrator",
                &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "XAI_API_KEY"],
            ));
        }

        Ok(Self::Orchestrated(Arc::new(orchestrator)))
    }

    #[must_use]
    pub fn provider_kind(&self) -> ProviderKind {
        match self {
            Self::ElaiApi(_) | Self::Orchestrated(_) => ProviderKind::ElaiApi,
            Self::Xai(_) => ProviderKind::Xai,
            Self::OpenAi(_) => ProviderKind::OpenAi,
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        match self {
            Self::ElaiApi(client) => send_via_provider(client, request).await,
            Self::Xai(client) | Self::OpenAi(client) => send_via_provider(client, request).await,
            Self::Orchestrated(orchestrator) => {
                orchestrator
                    .send_message(request, &RequestOptions::default())
                    .await
            }
        }
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        match self {
            Self::ElaiApi(client) => stream_via_provider(client, request)
                .await
                .map(MessageStream::ElaiApi),
            Self::Xai(client) | Self::OpenAi(client) => stream_via_provider(client, request)
                .await
                .map(MessageStream::OpenAiCompat),
            Self::Orchestrated(orchestrator) => {
                let response = orchestrator
                    .stream_message(request, &RequestOptions::default())
                    .await?;
                Ok(MessageStream::Collected(CollectedMessageStream::from_response(
                    response,
                )))
            }
        }
    }
}

#[derive(Debug)]
pub enum MessageStream {
    ElaiApi(elai_provider::MessageStream),
    OpenAiCompat(openai_compat::MessageStream),
    Collected(CollectedMessageStream),
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::ElaiApi(stream) => stream.request_id(),
            Self::OpenAiCompat(stream) => stream.request_id(),
            Self::Collected(stream) => stream.request_id(),
        }
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::ElaiApi(stream) => stream.next_event().await,
            Self::OpenAiCompat(stream) => stream.next_event().await,
            Self::Collected(stream) => stream.next_event().await,
        }
    }
}

/// Replays an already-collected `MessageResponse` as a synthetic event stream so
/// orchestrated calls can flow through the same `MessageStream` API.
#[derive(Debug)]
pub struct CollectedMessageStream {
    events: Vec<StreamEvent>,
    index: usize,
    request_id: Option<String>,
}

impl CollectedMessageStream {
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn from_response(response: MessageResponse) -> Self {
        let request_id = response.request_id.clone();
        let mut events = Vec::new();

        events.push(StreamEvent::MessageStart(MessageStartEvent {
            message: response.clone(),
        }));

        for (i, block) in response.content.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let index = i as u32;
            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index,
                content_block: block.clone(),
            }));
            if let OutputContentBlock::Text { text } = block {
                events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                    index,
                    delta: ContentBlockDelta::TextDelta { text: text.clone() },
                }));
            }
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index,
            }));
        }

        events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: MessageDelta {
                stop_reason: response.stop_reason.clone(),
                stop_sequence: response.stop_sequence.clone(),
            },
            usage: response.usage.clone(),
        }));
        events.push(StreamEvent::MessageStop(MessageStopEvent {}));

        Self {
            events,
            index: 0,
            request_id,
        }
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    #[allow(clippy::unused_async)]
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        if self.index < self.events.len() {
            let event = self.events[self.index].clone();
            self.index += 1;
            Ok(Some(event))
        } else {
            Ok(None)
        }
    }
}

pub use elai_provider::{
    oauth_token_is_expired, resolve_saved_oauth_token, resolve_startup_auth_source, OAuthTokenSet,
};
#[must_use]
pub fn read_base_url() -> String {
    elai_provider::read_base_url()
}

#[must_use]
pub fn read_xai_base_url() -> String {
    openai_compat::read_base_url(OpenAiCompatConfig::xai())
}

#[cfg(test)]
mod tests {
    use crate::providers::{detect_provider_kind, resolve_model_alias, ProviderKind};

    #[test]
    fn resolves_existing_and_grok_aliases() {
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
    }

    #[test]
    fn provider_detection_prefers_model_family() {
        assert_eq!(detect_provider_kind("grok-3"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::ElaiApi
        );
    }
}
