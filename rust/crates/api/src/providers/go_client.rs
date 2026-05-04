use super::elai_provider::{self, AuthSource, ElaiApiClient};
use super::openai_compat::{self, OpenAiCompatClient, OpenAiCompatConfig};
use super::{Provider, ProviderFuture};
use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse, StreamEvent};
use crate::types::{InputContentBlock, InputMessage};

pub const DEFAULT_GO_MESSAGES_BASE_URL: &str = "https://opencode.ai/zen/go";

#[derive(Debug, Clone)]
pub struct GoClient {
    chat_client: OpenAiCompatClient,
    messages_client: ElaiApiClient,
}

impl GoClient {
    pub fn from_env() -> Result<Self, ApiError> {
        let api_key = crate::providers::elai_provider::read_env_non_empty("OPENCODE_GO_API_KEY")?
            .ok_or_else(|| {
                ApiError::missing_credentials("OpenCode Go", &["OPENCODE_GO_API_KEY"])
            })?;

        let chat_base = read_go_chat_base_url();
        let messages_base = read_go_messages_base_url();

        Ok(Self {
            chat_client: OpenAiCompatClient::new(&api_key, OpenAiCompatConfig::opencode_go())
                .with_base_url(&chat_base),
            messages_client: ElaiApiClient::from_auth(AuthSource::BearerToken(api_key))
                .with_base_url(messages_base),
        })
    }

    #[must_use]
    pub fn with_chat_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.chat_client = self.chat_client.with_base_url(base_url);
        self
    }

    #[must_use]
    pub fn with_messages_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.messages_client = self.messages_client.with_base_url(base_url);
        self
    }

    fn is_messages_model(model: &str) -> bool {
        let lower = model.to_ascii_lowercase();
        lower == "minimax-m2.5" || lower == "minimax-m2.7"
    }

    fn reasoning_effort_for(model: &str) -> Option<String> {
        let lower = model.to_ascii_lowercase();
        if lower.contains("deepseek-v4") || lower.starts_with("kimi-k2") || lower.starts_with("glm-5") {
            Some("high".to_string())
        } else {
            None
        }
    }

    #[allow(clippy::unused_self)]
    fn prepare_request(&self, request: &MessageRequest) -> MessageRequest {
        let mut req = request.clone();
        if req.reasoning_effort.is_none() {
            req.reasoning_effort = Self::reasoning_effort_for(&request.model);
        }
        // Kimi e DeepSeek rejeitam mensagens terminando em tool result (400).
        // Anexa um `user: "."` sintético para desbloquear a continuação.
        if Self::needs_tool_continuation_bridge(&req) {
            req.messages.push(InputMessage {
                role: "user".to_string(),
                content: vec![InputContentBlock::Text {
                    text: ".".to_string(),
                }],
            });
        }
        req
    }

    fn needs_tool_continuation_bridge(request: &MessageRequest) -> bool {
        let model_lower = request.model.to_ascii_lowercase();
        let needs_bridge = model_lower.starts_with("kimi-k2")
            || model_lower.contains("deepseek-v4");
        needs_bridge
            && request
                .messages
                .last()
                .is_some_and(|msg| msg.role == "tool")
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        if Self::is_messages_model(&request.model) {
            self.messages_client.send_message(request).await
        } else {
            let req = self.prepare_request(request);
            self.chat_client.send_message(&req).await
        }
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        if Self::is_messages_model(&request.model) {
            self.messages_client
                .stream_message(request)
                .await
                .map(MessageStream::Messages)
        } else {
            let req = self.prepare_request(request);
            self.chat_client
                .stream_message(&req)
                .await
                .map(MessageStream::Chat)
        }
    }
}

impl Provider for GoClient {
    type Stream = MessageStream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse> {
        Box::pin(async move { self.send_message(request).await })
    }

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream> {
        Box::pin(async move { self.stream_message(request).await })
    }
}

#[derive(Debug)]
pub enum MessageStream {
    Chat(openai_compat::MessageStream),
    Messages(elai_provider::MessageStream),
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Chat(stream) => stream.request_id(),
            Self::Messages(stream) => stream.request_id(),
        }
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Chat(stream) => stream.next_event().await,
            Self::Messages(stream) => stream.next_event().await,
        }
    }
}

fn read_go_chat_base_url() -> String {
    std::env::var("OPENCODE_GO_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| openai_compat::DEFAULT_GO_CHAT_BASE_URL.to_string())
}

fn read_go_messages_base_url() -> String {
    std::env::var("OPENCODE_GO_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty()).map_or_else(|| DEFAULT_GO_MESSAGES_BASE_URL.to_string(), |url| url.trim_end_matches("/v1").trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::{GoClient, DEFAULT_GO_MESSAGES_BASE_URL};
    use crate::providers::openai_compat;

    #[test]
    fn minimax_m2_5_is_messages_model() {
        assert!(GoClient::is_messages_model("minimax-m2.5"));
    }

    #[test]
    fn minimax_m2_7_is_messages_model() {
        assert!(GoClient::is_messages_model("minimax-m2.7"));
    }

    #[test]
    fn other_models_are_chat_models() {
        assert!(!GoClient::is_messages_model("kimi-k2.6"));
        assert!(!GoClient::is_messages_model("glm-5"));
        assert!(!GoClient::is_messages_model("deepseek-v4-pro"));
        assert!(!GoClient::is_messages_model("qwen3.6-plus"));
        assert!(!GoClient::is_messages_model("mimo-v2-pro"));
    }

    #[test]
    fn messages_base_url_strips_v1_suffix() {
        assert_eq!(DEFAULT_GO_MESSAGES_BASE_URL, "https://opencode.ai/zen/go");
    }

    #[test]
    fn chat_base_url_has_v1_suffix() {
        assert_eq!(
            openai_compat::DEFAULT_GO_CHAT_BASE_URL,
            "https://opencode.ai/zen/go/v1"
        );
    }
}
