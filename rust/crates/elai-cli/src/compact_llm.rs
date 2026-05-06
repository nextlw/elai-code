//! `OpenAI`-only optional summarization for session compaction (`ELAI_COMPACT_MODEL`, default `gpt-4o-mini`).

use api::{
    InputMessage, MessageRequest, MessageResponse, OpenAiCompatClient, OpenAiCompatConfig,
    OutputContentBlock,
};
use runtime::{
    compact_session_with_summarizer, summarize_compact_excerpt, CompactionConfig, CompactionResult,
    ContentBlock, ConversationMessage, MessageRole, Session,
};

const DEFAULT_COMPACT_MODEL: &str = "gpt-4o-mini";
const MAX_EXCERPT_CHARS: usize = 120_000;

/// Env `ELAI_COMPACT_MODEL` overrides the `OpenAI` model used only for compaction summaries.
fn compact_model() -> String {
    std::env::var("ELAI_COMPACT_MODEL").unwrap_or_else(|_| DEFAULT_COMPACT_MODEL.to_string())
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "\n[truncated excerpt for summarization]"
}

fn format_messages_for_summary(messages: &[ConversationMessage]) -> String {
    let mut out = String::new();
    for msg in messages {
        let role = match msg.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let mut parts = Vec::new();
        for block in &msg.blocks {
            match block {
                ContentBlock::Text { text } => parts.push(text.clone()),
                ContentBlock::Image { media_type, .. } => {
                    parts.push(format!("[image {media_type}]"));
                }
                ContentBlock::Document { media_type, name, .. } => {
                    parts.push(format!("[document {} ({media_type})]", name.as_deref().unwrap_or("anexo")));
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    parts.push(format!("tool_use {name} {input}"));
                }
                ContentBlock::ToolResult {
                    tool_use_id: _,
                    tool_name,
                    output,
                    is_error,
                } => {
                    let prefix = if *is_error {
                        "tool_result error "
                    } else {
                        "tool_result "
                    };
                    parts.push(format!("{prefix}{tool_name}: {output}"));
                }
                ContentBlock::Thinking { .. } => {}
            }
        }
        let body = parts.join("\n");
        out.push_str(role);
        out.push_str(": ");
        out.push_str(&body);
        out.push('\n');
    }
    truncate_chars(&out, MAX_EXCERPT_CHARS)
}

fn extract_summary_text(response: &MessageResponse) -> Option<String> {
    let mut chunks = Vec::new();
    for block in &response.content {
        if let OutputContentBlock::Text { text } = block {
            chunks.push(text.as_str());
        }
    }
    let joined = chunks.join("\n").trim().to_string();
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Calls `OpenAI` Chat Completions (official endpoint via [`OpenAiCompatConfig::openai`]) to summarize
/// removed messages. Returns `None` if credentials are missing or the request fails.
pub fn try_openai_compact_summary(removed: &[ConversationMessage]) -> Option<String> {
    let client = OpenAiCompatClient::from_env(OpenAiCompatConfig::openai()).ok()?;
    let body = format_messages_for_summary(removed);
    let rt = tokio::runtime::Runtime::new().ok()?;
    let model = compact_model();
    let request = MessageRequest {
        model,
        max_tokens: 4096,
        messages: vec![InputMessage::user_text(format!(
            "Summarize this conversation excerpt for a coding assistant context window. \
Preserve file paths, decisions, pending tasks, and tool names. Use concise structured text.\n\n---\n{body}"
        ))],
        system: Some(
            "You compress assistant conversation history into a dense summary for context compaction."
                .to_string(),
        ),
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: None,
        output_config: None,
        reasoning_effort: None,
    };
    let response = rt.block_on(client.send_message(&request)).ok()?;
    extract_summary_text(&response)
}

/// Runs compaction using an `OpenAI` summary when credentials exist; otherwise deterministic [`summarize_compact_excerpt`].
pub fn compact_session_with_optional_openai(
    session: &Session,
    config: CompactionConfig,
) -> CompactionResult {
    compact_session_with_summarizer(session, config, |removed| {
        try_openai_compact_summary(removed).unwrap_or_else(|| summarize_compact_excerpt(removed))
    })
}
