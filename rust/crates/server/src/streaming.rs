use serde::{Deserialize, Serialize};

/// Canonical SSE event published to clients.
///
/// `seq` is monotonic per session and used for `?since=N` reconnection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Snapshot {
        seq: u64,
        session_id: String,
        session: SessionSnapshot,
    },
    TurnStarted {
        seq: u64,
        session_id: String,
        turn_id: String,
    },
    TextDelta {
        seq: u64,
        session_id: String,
        turn_id: String,
        text: String,
    },
    ThinkingDelta {
        seq: u64,
        session_id: String,
        turn_id: String,
        thinking: String,
    },
    ToolUseStarted {
        seq: u64,
        session_id: String,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
    },
    ToolUseInputDelta {
        seq: u64,
        session_id: String,
        turn_id: String,
        tool_call_id: String,
        partial_json: String,
    },
    ToolResult {
        seq: u64,
        session_id: String,
        turn_id: String,
        tool_call_id: String,
        output: String,
        is_error: bool,
    },
    PermissionRequest {
        seq: u64,
        session_id: String,
        request_id: String,
        tool_name: String,
        input: String,
        required_mode: String,
    },
    UsageDelta {
        seq: u64,
        session_id: String,
        turn_id: String,
        input_tokens: u32,
        output_tokens: u32,
    },
    TurnCompleted {
        seq: u64,
        session_id: String,
        turn_id: String,
    },
    TurnError {
        seq: u64,
        session_id: String,
        turn_id: String,
        error: String,
    },
    TurnCancelled {
        seq: u64,
        session_id: String,
        turn_id: String,
    },
    MessageAppended {
        seq: u64,
        session_id: String,
        role: String,
        text_summary: String,
    },
}

impl ServerEvent {
    #[must_use]
    pub fn seq(&self) -> u64 {
        match self {
            Self::Snapshot { seq, .. }
            | Self::TurnStarted { seq, .. }
            | Self::TextDelta { seq, .. }
            | Self::ThinkingDelta { seq, .. }
            | Self::ToolUseStarted { seq, .. }
            | Self::ToolUseInputDelta { seq, .. }
            | Self::ToolResult { seq, .. }
            | Self::PermissionRequest { seq, .. }
            | Self::UsageDelta { seq, .. }
            | Self::TurnCompleted { seq, .. }
            | Self::TurnError { seq, .. }
            | Self::TurnCancelled { seq, .. }
            | Self::MessageAppended { seq, .. } => *seq,
        }
    }

    #[must_use]
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Snapshot { .. } => "snapshot",
            Self::TurnStarted { .. } => "turn_started",
            Self::TextDelta { .. } => "text_delta",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ToolUseStarted { .. } => "tool_use_started",
            Self::ToolUseInputDelta { .. } => "tool_use_input_delta",
            Self::ToolResult { .. } => "tool_result",
            Self::PermissionRequest { .. } => "permission_request",
            Self::UsageDelta { .. } => "usage_delta",
            Self::TurnCompleted { .. } => "turn_completed",
            Self::TurnError { .. } => "turn_error",
            Self::TurnCancelled { .. } => "turn_cancelled",
            Self::MessageAppended { .. } => "message_appended",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub created_at: u64,
    pub model: String,
    pub permission_mode: String,
    pub cwd: String,
    pub message_count: usize,
}
