use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;

use super::types::ProviderCapability;
use crate::error::ApiError;
use crate::providers::{codex_bridge, elai_provider, go_client, openai_compat};
use crate::types::{
    ContentBlockDelta, MessageResponse, MessageRequest, OutputContentBlock, StreamEvent, Usage,
};

#[async_trait]
pub trait UnifiedProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> &HashSet<ProviderCapability>;
    async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError>;
    async fn stream_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError>;
    async fn health_check(&self) -> Result<(), ApiError>;
}

/// Adapter wrapping `ElaiApiClient` so it conforms to `UnifiedProvider`.
pub struct ElaiUnifiedAdapter {
    client: elai_provider::ElaiApiClient,
    provider_id: String,
    capabilities: HashSet<ProviderCapability>,
}

impl ElaiUnifiedAdapter {
    #[must_use]
    pub fn new(client: elai_provider::ElaiApiClient) -> Self {
        let mut caps = HashSet::new();
        caps.insert(ProviderCapability::Thinking);
        caps.insert(ProviderCapability::ToolCalling);
        caps.insert(ProviderCapability::Streaming);
        Self {
            client,
            provider_id: "anthropic".to_string(),
            capabilities: caps,
        }
    }
}

#[async_trait]
impl UnifiedProvider for ElaiUnifiedAdapter {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn capabilities(&self) -> &HashSet<ProviderCapability> {
        &self.capabilities
    }

    async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        self.client.send_message(request).await
    }

    async fn stream_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        let mut stream_req = request.clone();
        stream_req.stream = true;
        let mut stream = self.client.stream_message(&stream_req).await?;
        collect_elai_stream(&mut stream).await
    }

    async fn health_check(&self) -> Result<(), ApiError> {
        Ok(())
    }
}

/// Adapter wrapping `OpenAiCompatClient` so it conforms to `UnifiedProvider`.
pub struct OpenAiUnifiedAdapter {
    client: openai_compat::OpenAiCompatClient,
    provider_id: String,
    capabilities: HashSet<ProviderCapability>,
}

impl OpenAiUnifiedAdapter {
    #[must_use]
    pub fn new(client: openai_compat::OpenAiCompatClient, id: &str) -> Self {
        let mut caps = HashSet::new();
        caps.insert(ProviderCapability::ToolCalling);
        caps.insert(ProviderCapability::Streaming);
        Self {
            client,
            provider_id: id.to_string(),
            capabilities: caps,
        }
    }
}

#[async_trait]
impl UnifiedProvider for OpenAiUnifiedAdapter {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn capabilities(&self) -> &HashSet<ProviderCapability> {
        &self.capabilities
    }

    async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        self.client.send_message(request).await
    }

    async fn stream_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        let mut stream_req = request.clone();
        stream_req.stream = true;
        let mut stream = self.client.stream_message(&stream_req).await?;
        collect_openai_stream(&mut stream).await
    }

    async fn health_check(&self) -> Result<(), ApiError> {
        Ok(())
    }
}

/// Adapter wrapping `CodexBridgeClient` so orchestrator can delegate `OpenAI`
/// requests through local `codex exec` when using `ChatGPT` auth.
pub struct CodexBridgeUnifiedAdapter {
    client: codex_bridge::CodexBridgeClient,
    provider_id: String,
    capabilities: HashSet<ProviderCapability>,
}

impl CodexBridgeUnifiedAdapter {
    #[must_use]
    pub fn new(client: codex_bridge::CodexBridgeClient, id: &str) -> Self {
        let mut caps = HashSet::new();
        caps.insert(ProviderCapability::Streaming);
        Self {
            client,
            provider_id: id.to_string(),
            capabilities: caps,
        }
    }
}

#[async_trait]
impl UnifiedProvider for CodexBridgeUnifiedAdapter {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn capabilities(&self) -> &HashSet<ProviderCapability> {
        &self.capabilities
    }

    async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        self.client.send_message(request).await
    }

    async fn stream_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        let mut stream_req = request.clone();
        stream_req.stream = true;
        let mut stream = self.client.stream_message(&stream_req).await?;
        collect_codex_bridge_stream(&mut stream).await
    }

    async fn health_check(&self) -> Result<(), ApiError> {
        Ok(())
    }
}

/// Adapter wrapping `GoClient` so orchestrator can use `OpenCode` Go models.
pub struct GoUnifiedAdapter {
    client: go_client::GoClient,
    provider_id: String,
    capabilities: HashSet<ProviderCapability>,
}

impl GoUnifiedAdapter {
    #[must_use]
    pub fn new(client: go_client::GoClient) -> Self {
        let mut caps = HashSet::new();
        caps.insert(ProviderCapability::ToolCalling);
        caps.insert(ProviderCapability::Streaming);
        Self {
            client,
            provider_id: "opencode-go".to_string(),
            capabilities: caps,
        }
    }
}

#[async_trait]
impl UnifiedProvider for GoUnifiedAdapter {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn capabilities(&self) -> &HashSet<ProviderCapability> {
        &self.capabilities
    }

    async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        self.client.send_message(request).await
    }

    async fn stream_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        let mut stream_req = request.clone();
        stream_req.stream = true;
        let mut stream = self.client.stream_message(&stream_req).await?;
        collect_go_stream(&mut stream).await
    }

    async fn health_check(&self) -> Result<(), ApiError> {
        Ok(())
    }
}

fn empty_response() -> MessageResponse {
    MessageResponse {
        id: String::new(),
        kind: "message".to_string(),
        role: "assistant".to_string(),
        content: Vec::new(),
        model: String::new(),
        stop_reason: None,
        stop_sequence: None,
        usage: Usage {
            input_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens: 0,
        },
        request_id: None,
    }
}

async fn collect_elai_stream(
    stream: &mut elai_provider::MessageStream,
) -> Result<MessageResponse, ApiError> {
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await? {
        events.push(event);
    }
    Ok(collect_stream_events(events))
}

async fn collect_openai_stream(
    stream: &mut openai_compat::MessageStream,
) -> Result<MessageResponse, ApiError> {
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await? {
        events.push(event);
    }
    Ok(collect_stream_events(events))
}

async fn collect_codex_bridge_stream(
    stream: &mut codex_bridge::MessageStream,
) -> Result<MessageResponse, ApiError> {
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await? {
        events.push(event);
    }
    Ok(collect_stream_events(events))
}

async fn collect_go_stream(
    stream: &mut go_client::MessageStream,
) -> Result<MessageResponse, ApiError> {
    let mut events = Vec::new();
    while let Some(event) = stream.next_event().await? {
        events.push(event);
    }
    Ok(collect_stream_events(events))
}

fn collect_stream_events(events: Vec<StreamEvent>) -> MessageResponse {
    let mut response: Option<MessageResponse> = None;
    let mut content_texts: Vec<(u32, String)> = Vec::new();
    // index → (id, name, accumulated_json)
    let mut pending_tool_uses: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
    let mut finalized_tool_uses: Vec<(u32, String, String, String)> = Vec::new();
    // index → accumulated thinking text
    let mut pending_thinking: BTreeMap<u32, String> = BTreeMap::new();
    let mut stop_reason: Option<String> = None;
    let mut stop_sequence: Option<String> = None;
    let mut usage: Option<Usage> = None;

    for event in events {
        match event {
            StreamEvent::MessageStart(e) => {
                response = Some(e.message);
            }
            StreamEvent::ContentBlockStart(e) => {
                match &e.content_block {
                    OutputContentBlock::Text { .. } => {
                        content_texts.push((e.index, String::new()));
                    }
                    OutputContentBlock::ToolUse { id, name, .. } => {
                        pending_tool_uses
                            .insert(e.index, (id.clone(), name.clone(), String::new()));
                    }
                    OutputContentBlock::Thinking { .. } => {
                        pending_thinking.insert(e.index, String::new());
                    }
                    OutputContentBlock::RedactedThinking { .. } => {}
                }
            }
            StreamEvent::ContentBlockDelta(e) => match e.delta {
                ContentBlockDelta::TextDelta { text } => {
                    if let Some(entry) = content_texts.iter_mut().find(|(i, _)| *i == e.index) {
                        entry.1.push_str(&text);
                    }
                }
                ContentBlockDelta::InputJsonDelta { partial_json } => {
                    if let Some(entry) = pending_tool_uses.get_mut(&e.index) {
                        entry.2.push_str(&partial_json);
                    }
                }
                ContentBlockDelta::ThinkingDelta { thinking } => {
                    if let Some(entry) = pending_thinking.get_mut(&e.index) {
                        entry.push_str(&thinking);
                    }
                }
                ContentBlockDelta::SignatureDelta { .. } => {}
            },
            StreamEvent::ContentBlockStop(e) => {
                if let Some((id, name, json)) = pending_tool_uses.remove(&e.index) {
                    finalized_tool_uses.push((e.index, id, name, json));
                }
            }
            StreamEvent::MessageDelta(e) => {
                stop_reason = e.delta.stop_reason;
                stop_sequence = e.delta.stop_sequence;
                usage = Some(e.usage);
            }
            StreamEvent::MessageStop(_) => {}
        }
    }

    let mut resp = response.unwrap_or_else(empty_response);

    // Thinking blocks come first in the content array so they precede text/tool_uses.
    let thinking_text: String = pending_thinking.into_values().collect::<Vec<_>>().concat();
    let mut content: Vec<OutputContentBlock> = Vec::new();
    if !thinking_text.is_empty() {
        content.push(OutputContentBlock::Thinking { thinking: thinking_text, signature: None });
    }

    content.extend(
        content_texts
            .into_iter()
            .map(|(_, text)| OutputContentBlock::Text { text }),
    );

    finalized_tool_uses.sort_by_key(|(index, _, _, _)| *index);
    for (_, id, name, json) in finalized_tool_uses {
        let input = serde_json::from_str(&json).unwrap_or_else(|_| serde_json::json!({}));
        content.push(OutputContentBlock::ToolUse { id, name, input });
    }

    resp.content = content;
    if let Some(sr) = stop_reason {
        resp.stop_reason = Some(sr);
    }
    if let Some(ss) = stop_sequence {
        resp.stop_sequence = Some(ss);
    }
    if let Some(u) = usage {
        resp.usage = u;
    }
    resp
}
