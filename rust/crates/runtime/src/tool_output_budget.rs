//! Truncates large tool result strings before they are sent to the model
//! (equivalent to a light `applyToolResultBudget` in the reference pipeline).
//! Config: `ELAI_TOOL_OUTPUT_MAX_CHARS` (default 80_000 characters per tool result block).

use crate::session::{ContentBlock, ConversationMessage};

const DEFAULT_MAX_TOOL_OUTPUT_CHARS: usize = 80_000;

fn max_chars() -> usize {
    std::env::var("ELAI_TOOL_OUTPUT_MAX_CHARS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_TOOL_OUTPUT_CHARS)
}

/// Truncates each `ToolResult` block output that exceeds the configured cap.
pub fn apply_tool_result_budget(messages: &mut [ConversationMessage]) {
    let max = max_chars();
    for msg in messages.iter_mut() {
        for block in msg.blocks.iter_mut() {
            if let ContentBlock::ToolResult { output, .. } = block {
                let n = output.chars().count();
                if n > max {
                    let truncated: String = output.chars().take(max).collect();
                    *output = format!(
                        "{truncated}\n\n[truncated: output exceeded {max} characters; set ELAI_TOOL_OUTPUT_MAX_CHARS to raise]"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply_tool_result_budget;
    use crate::session::{ContentBlock, ConversationMessage, MessageRole};

    #[test]
    fn truncates_oversized_tool_result() {
        let huge = "x".repeat(100);
        std::env::set_var("ELAI_TOOL_OUTPUT_MAX_CHARS", "50");
        let mut messages = vec![ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                tool_name: "read_file".to_string(),
                output: huge,
                is_error: false,
            }],
            usage: None,
        }];
        apply_tool_result_budget(&mut messages);
        let ContentBlock::ToolResult { output, .. } = &messages[0].blocks[0] else {
            panic!();
        };
        assert!(output.contains("[truncated:"));
        assert!(output.len() < 200);
        std::env::remove_var("ELAI_TOOL_OUTPUT_MAX_CHARS");
    }
}
