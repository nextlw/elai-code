use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Thinking / Effort Configuration ──────────────────────────
// Mirrors the Anthropic Messages API `thinking` parameter.
// - `Adaptive`: the model decides when and how much to think.
// - `Enabled { budget_tokens }`: always think, with an explicit token budget.
// - `Disabled`: suppress extended thinking entirely.
//
// See: https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    /// The model dynamically decides thinking depth per turn.
    Adaptive,
    /// Always think, capped at `budget_tokens` thinking tokens.
    Enabled { budget_tokens: u32 },
    /// No extended thinking.
    Disabled,
}

/// Controls the overall effort level the model applies to the response.
/// Maps to `output_config.effort` in the Anthropic API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortLevel {
    High,
    Medium,
    Low,
}

/// Wrapper for the `output_config` request field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputConfig {
    pub effort: EffortLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<InputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    /// Extended thinking configuration. When `Some`, the API is asked to
    /// expose its chain-of-thought reasoning before producing the final answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// Output effort configuration. When `Some`, controls how much effort
    /// the model applies (high = thorough, low = fast/cheap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    /// Reasoning effort for OpenAI-compatible models (`DeepSeek`, Kimi, GLM, etc.).
    /// Values: `"low"`, `"medium"`, `"high"`, `"max"` (deepseek-v4 only).
    /// Only sent when `Some` — the field is omitted from JSON otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl MessageRequest {
    #[must_use]
    pub fn with_streaming(mut self) -> Self {
        self.stream = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputMessage {
    pub role: String,
    pub content: Vec<InputContentBlock>,
}

impl InputMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text { text: text.into() }],
        }
    }

    #[must_use]
    pub fn user_tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: vec![ToolResultContentBlock::Text {
                    text: content.into(),
                }],
                is_error,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ToolResultContentBlock>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    Thinking {
        thinking: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContentBlock {
    Text { text: String },
    Json { value: Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub role: String,
    pub content: Vec<OutputContentBlock>,
    pub model: String,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
    pub usage: Usage,
    #[serde(default)]
    pub request_id: Option<String>,
}

impl MessageResponse {
    #[must_use]
    pub fn total_tokens(&self) -> u32 {
        self.usage.total_tokens()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    pub output_tokens: u32,
}

impl Usage {
    #[must_use]
    pub const fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageStartEvent {
    pub message: MessageResponse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaEvent {
    pub delta: MessageDelta,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDelta {
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockStartEvent {
    pub index: u32,
    pub content_block: OutputContentBlock,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockDeltaEvent {
    pub index: u32,
    pub delta: ContentBlockDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentBlockStopEvent {
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageStopEvent {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart(MessageStartEvent),
    MessageDelta(MessageDeltaEvent),
    ContentBlockStart(ContentBlockStartEvent),
    ContentBlockDelta(ContentBlockDeltaEvent),
    ContentBlockStop(ContentBlockStopEvent),
    MessageStop(MessageStopEvent),
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thinking_config_adaptive_serializes_correctly() {
        let config = ThinkingConfig::Adaptive;
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, json!({"type": "adaptive"}));
    }

    #[test]
    fn thinking_config_enabled_serializes_correctly() {
        let config = ThinkingConfig::Enabled { budget_tokens: 10_000 };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, json!({"type": "enabled", "budget_tokens": 10000}));
    }

    #[test]
    fn thinking_config_disabled_serializes_correctly() {
        let config = ThinkingConfig::Disabled;
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, json!({"type": "disabled"}));
    }

    #[test]
    fn thinking_config_round_trips() {
        for config in [
            ThinkingConfig::Adaptive,
            ThinkingConfig::Enabled { budget_tokens: 32_000 },
            ThinkingConfig::Disabled,
        ] {
            let json = serde_json::to_string(&config).unwrap();
            let parsed: ThinkingConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, config);
        }
    }

    #[test]
    fn effort_level_serializes_as_snake_case() {
        assert_eq!(serde_json::to_value(EffortLevel::High).unwrap(), json!("high"));
        assert_eq!(serde_json::to_value(EffortLevel::Medium).unwrap(), json!("medium"));
        assert_eq!(serde_json::to_value(EffortLevel::Low).unwrap(), json!("low"));
    }

    #[test]
    fn output_config_serializes_correctly() {
        let config = OutputConfig { effort: EffortLevel::High };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, json!({"effort": "high"}));
    }

    #[test]
    fn message_request_omits_thinking_when_none() {
        let request = MessageRequest {
            model: "test".to_string(),
            max_tokens: 100,
            messages: vec![],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            reasoning_effort: None,
            output_config: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("thinking").is_none());
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn message_request_includes_thinking_when_set() {
        let request = MessageRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 8192,
            messages: vec![],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: Some(ThinkingConfig::Adaptive),
            reasoning_effort: None,
            output_config: Some(OutputConfig { effort: EffortLevel::High }),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["thinking"], json!({"type": "adaptive"}));
        assert_eq!(json["output_config"], json!({"effort": "high"}));
    }
}
