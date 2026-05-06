use api::{
    max_tokens_for_model, resolve_model_alias, InputContentBlock, InputMessage, MessageRequest,
    OutputContentBlock, ProviderClient, ToolChoice, ToolResultContentBlock,
};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage, MessageRole,
    RuntimeError, TokenUsage, ToolError, ToolExecutor,
};
use tools::GlobalToolRegistry;

/// Bridges the runtime's sync `ApiClient` trait to the async `ProviderClient`.
///
/// Owns its own tokio runtime for `block_on`, mirroring the CLI's
/// `DefaultRuntimeClient` pattern (the runtime trait is sync but provider HTTP is async).
pub struct ServerApiClient {
    inner_rt: tokio::runtime::Runtime,
    client: ProviderClient,
    model: String,
    tool_registry: GlobalToolRegistry,
}

impl ServerApiClient {
    pub fn new(model: String, tool_registry: GlobalToolRegistry) -> Result<Self, String> {
        let client = ProviderClient::from_model(&model).map_err(|error| error.to_string())?;
        let inner_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            inner_rt,
            client,
            model,
            tool_registry,
        })
    }
}

impl ApiClient for ServerApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let resolved = resolve_model_alias(&self.model);
        let tool_defs = self.tool_registry.definitions(None);
        let message_request = MessageRequest {
            model: resolved.clone(),
            max_tokens: max_tokens_for_model(&resolved),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: (!tool_defs.is_empty()).then_some(tool_defs),
            tool_choice: Some(ToolChoice::Auto),
            stream: false,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
        };

        let response = self
            .inner_rt
            .block_on(self.client.send_message(&message_request))
            .map_err(|error| RuntimeError::new(error.to_string()))?;

        let mut events = Vec::new();
        for block in response.content {
            match block {
                OutputContentBlock::Text { text } => {
                    if !text.is_empty() {
                        events.push(AssistantEvent::TextDelta(text));
                    }
                }
                OutputContentBlock::ToolUse { id, name, input } => {
                    let input_str = serde_json::to_string(&input)
                        .unwrap_or_else(|_| "{}".to_string());
                    events.push(AssistantEvent::ToolUse {
                        id,
                        name,
                        input: input_str,
                    });
                }
                OutputContentBlock::Thinking { thinking, .. } => {
                    if !thinking.is_empty() {
                        events.push(AssistantEvent::Thinking { thinking });
                    }
                }
                OutputContentBlock::RedactedThinking { .. } => {}
            }
        }
        events.push(AssistantEvent::Usage(TokenUsage {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
            cache_read_input_tokens: response.usage.cache_read_input_tokens,
        }));
        events.push(AssistantEvent::MessageStop);
        Ok(events)
    }
}

fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };
            let content: Vec<InputContentBlock> = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => {
                        Some(InputContentBlock::Text { text: text.clone() })
                    }
                    ContentBlock::ToolUse { id, name, input } => Some(InputContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::from_str(input)
                            .unwrap_or_else(|_| serde_json::json!({ "raw": input })),
                    }),
                    ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => Some(InputContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![ToolResultContentBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    }),
                    ContentBlock::Thinking { thinking } => Some(InputContentBlock::Thinking {
                        thinking: thinking.clone(),
                    }),
                    // Server bridge é text-only por enquanto — anexos são
                    // descartados; o runtime já gateia multimodal via
                    // `supports_multimodal()` antes de chegar aqui.
                    ContentBlock::Image { .. } | ContentBlock::Document { .. } => None,
                })
                .collect();
            (!content.is_empty()).then(|| InputMessage {
                role: role.to_string(),
                content,
            })
        })
        .collect()
}

/// Wraps a `GlobalToolRegistry` to satisfy the runtime's sync `ToolExecutor` trait.
pub struct ServerToolExecutor {
    registry: GlobalToolRegistry,
}

impl ServerToolExecutor {
    #[must_use]
    pub fn new(registry: GlobalToolRegistry) -> Self {
        Self { registry }
    }
}

impl ToolExecutor for ServerToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let parsed: serde_json::Value =
            serde_json::from_str(input).unwrap_or(serde_json::json!({}));
        self.registry
            .execute(tool_name, &parsed)
            .map_err(ToolError::new)
    }
}
