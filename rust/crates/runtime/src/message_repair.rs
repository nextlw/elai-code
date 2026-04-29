use std::collections::HashSet;

use crate::session::{ContentBlock, ConversationMessage, MessageRole};

/// Describes each repair action taken. Useful for logging and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairAction {
    /// A tool_use had no corresponding tool_result — injected synthetic.
    InjectedSyntheticToolResult { tool_use_id: String },
    /// A tool_result had no corresponding tool_use — removed.
    RemovedOrphanedToolResult { tool_use_id: String },
    /// First message was not User — inserted placeholder.
    PrependedUserPlaceholder,
}

/// Validates and repairs a message sequence to guarantee Anthropic API invariants.
/// Returns the list of actions taken (empty = no issues found).
///
/// Should be called immediately before constructing ApiRequest.
/// Does NOT modify the persisted session — only the cloned messages sent to the API.
pub fn validate_and_repair(messages: &mut Vec<ConversationMessage>) -> Vec<RepairAction> {
    let mut actions = Vec::new();
    repair_orphaned_tool_results(messages, &mut actions);
    repair_missing_tool_results(messages, &mut actions);
    repair_first_message_role(messages, &mut actions);
    actions
}

/// Collects all tool_use ids present across all messages, then removes any
/// ToolResult blocks whose tool_use_id has no corresponding ToolUse block.
fn repair_orphaned_tool_results(messages: &mut Vec<ConversationMessage>, actions: &mut Vec<RepairAction>) {
    // Collect every tool_use id that exists.
    let known_tool_use_ids: HashSet<String> = messages
        .iter()
        .flat_map(|msg| &msg.blocks)
        .filter_map(|block| {
            if let ContentBlock::ToolUse { id, .. } = block {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();

    // For each message, remove ToolResult blocks whose id is not in the known set.
    for msg in messages.iter_mut() {
        let orphaned_ids: Vec<String> = msg
            .blocks
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    if !known_tool_use_ids.contains(tool_use_id) {
                        return Some(tool_use_id.clone());
                    }
                }
                None
            })
            .collect();

        if orphaned_ids.is_empty() {
            continue;
        }

        let orphaned_set: HashSet<&String> = orphaned_ids.iter().collect();
        msg.blocks.retain(|block| {
            if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                !orphaned_set.contains(tool_use_id)
            } else {
                true
            }
        });

        for id in orphaned_ids {
            actions.push(RepairAction::RemovedOrphanedToolResult { tool_use_id: id });
        }
    }

    // Remove messages that became empty after block removal.
    messages.retain(|msg| !msg.blocks.is_empty());
}

/// For each Assistant message containing ToolUse blocks, verifies the next
/// message contains ToolResult blocks for each tool_use_id. If not, injects
/// a synthetic Tool message with a placeholder result.
fn repair_missing_tool_results(messages: &mut Vec<ConversationMessage>, actions: &mut Vec<RepairAction>) {
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != MessageRole::Assistant {
            i += 1;
            continue;
        }

        // Collect tool_use ids from this assistant message.
        let tool_use_ids: Vec<String> = messages[i]
            .blocks
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolUse { id, .. } = block {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        if tool_use_ids.is_empty() {
            i += 1;
            continue;
        }

        // Collect tool_use_ids that are already covered by the next message.
        let covered: HashSet<String> = messages
            .get(i + 1)
            .map(|next_msg| {
                next_msg
                    .blocks
                    .iter()
                    .filter_map(|block| {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                            Some(tool_use_id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Find ids that need synthetic results.
        let missing_ids: Vec<String> = tool_use_ids
            .into_iter()
            .filter(|id| !covered.contains(id))
            .collect();

        if missing_ids.is_empty() {
            i += 1;
            continue;
        }

        // Build a single synthetic Tool message containing all missing results.
        let synthetic_blocks: Vec<ContentBlock> = missing_ids
            .iter()
            .map(|id| ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                tool_name: "unknown".to_string(),
                output: "[Result unavailable — conversation was compacted]".to_string(),
                is_error: true,
            })
            .collect();

        let synthetic_message = ConversationMessage {
            role: MessageRole::Tool,
            blocks: synthetic_blocks,
            usage: None,
        };

        // Insert the synthetic message right after the assistant message.
        messages.insert(i + 1, synthetic_message);

        for id in missing_ids {
            actions.push(RepairAction::InjectedSyntheticToolResult { tool_use_id: id });
        }

        // Skip past both the assistant message and the newly inserted tool message.
        i += 2;
    }
}

/// Ensures the first non-System message is a valid User message.
///
/// Iteratively removes leading Tool messages and User messages that contain
/// only ToolResult blocks (both are invalid as the first API message). After
/// draining any such messages, prepends a placeholder User message if needed.
/// Uses a loop instead of recursion to avoid duplicate action entries.
fn repair_first_message_role(messages: &mut Vec<ConversationMessage>, actions: &mut Vec<RepairAction>) {
    loop {
        let idx = match messages.iter().position(|m| m.role != MessageRole::System) {
            Some(i) => i,
            None => return,
        };

        let msg = &messages[idx];

        // A User message that has at least one non-ToolResult block is valid — stop.
        if msg.role == MessageRole::User {
            let has_non_tool_result = msg.blocks.iter().any(|b| !matches!(b, ContentBlock::ToolResult { .. }));
            if has_non_tool_result {
                return;
            }
        }

        // Tool messages and User messages with only ToolResult blocks are invalid as first.
        let is_invalid_first = msg.role == MessageRole::Tool
            || (!msg.blocks.is_empty()
                && msg.blocks.iter().all(|b| matches!(b, ContentBlock::ToolResult { .. })));

        if is_invalid_first {
            messages.remove(idx);
            continue;
        }

        // First non-System message has wrong role (e.g. Assistant) — prepend placeholder.
        if msg.role != MessageRole::User {
            messages.insert(idx, ConversationMessage::user_text("[Conversation resumed]"));
            actions.push(RepairAction::PrependedUserPlaceholder);
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ContentBlock, ConversationMessage, MessageRole};

    fn make_user(text: &str) -> ConversationMessage {
        ConversationMessage::user_text(text)
    }

    fn make_assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: text.to_string(),
        }])
    }

    fn make_assistant_with_tool_use(tool_use_id: &str) -> ConversationMessage {
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: tool_use_id.to_string(),
            name: "bash".to_string(),
            input: "echo hi".to_string(),
        }])
    }

    fn make_tool_result(tool_use_id: &str) -> ConversationMessage {
        ConversationMessage::tool_result(tool_use_id, "bash", "hi", false)
    }

    #[test]
    fn no_repairs_for_valid_sequence() {
        let mut messages = vec![
            make_user("hello"),
            make_assistant_with_tool_use("tool-1"),
            make_tool_result("tool-1"),
            make_assistant_text("done"),
        ];
        let actions = validate_and_repair(&mut messages);
        assert!(actions.is_empty(), "expected no repairs for valid sequence: {actions:?}");
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn repairs_tool_use_without_tool_result() {
        let mut messages = vec![
            make_user("hello"),
            make_assistant_with_tool_use("tool-1"),
            // No tool_result following the assistant message.
        ];
        let actions = validate_and_repair(&mut messages);

        assert_eq!(
            actions,
            vec![RepairAction::InjectedSyntheticToolResult {
                tool_use_id: "tool-1".to_string()
            }]
        );

        // The synthetic message should have been inserted after the assistant message.
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].role, MessageRole::Tool);
        let block = &messages[2].blocks[0];
        assert!(
            matches!(block, ContentBlock::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "tool-1"),
            "expected synthetic tool result block, got: {block:?}"
        );
    }

    #[test]
    fn repairs_orphaned_tool_result() {
        // tool_result references "nonexistent-id" which has no corresponding tool_use.
        let mut messages = vec![
            make_user("hello"),
            ConversationMessage {
                role: MessageRole::Tool,
                blocks: vec![ContentBlock::ToolResult {
                    tool_use_id: "nonexistent-id".to_string(),
                    tool_name: "bash".to_string(),
                    output: "output".to_string(),
                    is_error: false,
                }],
                usage: None,
            },
            make_assistant_text("done"),
        ];
        let actions = validate_and_repair(&mut messages);

        assert!(
            actions.iter().any(|a| matches!(a, RepairAction::RemovedOrphanedToolResult { tool_use_id } if tool_use_id == "nonexistent-id")),
            "expected RemovedOrphanedToolResult action: {actions:?}"
        );

        // The orphaned tool result message should have been removed.
        for msg in &messages {
            for block in &msg.blocks {
                assert!(
                    !matches!(block, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "nonexistent-id"),
                    "orphaned tool result block should have been removed"
                );
            }
        }
    }

    #[test]
    fn repairs_sequence_starting_with_tool_result() {
        // First message is a Tool/ToolResult with no preceding ToolUse.
        let mut messages = vec![
            ConversationMessage {
                role: MessageRole::Tool,
                blocks: vec![ContentBlock::ToolResult {
                    tool_use_id: "orphan-id".to_string(),
                    tool_name: "bash".to_string(),
                    output: "output".to_string(),
                    is_error: false,
                }],
                usage: None,
            },
            make_assistant_text("done"),
        ];
        let actions = validate_and_repair(&mut messages);

        // The orphaned tool result should be removed and a user placeholder prepended.
        assert!(
            actions.iter().any(|a| matches!(a, RepairAction::RemovedOrphanedToolResult { .. })),
            "expected RemovedOrphanedToolResult: {actions:?}"
        );
        assert!(
            actions.iter().any(|a| matches!(a, RepairAction::PrependedUserPlaceholder)),
            "expected PrependedUserPlaceholder: {actions:?}"
        );

        // Sequence should now start with a User message.
        let first_non_system = messages
            .iter()
            .find(|msg| msg.role != MessageRole::System)
            .expect("should have at least one message");
        assert_eq!(
            first_non_system.role,
            MessageRole::User,
            "first non-system message should be User after repair"
        );
    }

    #[test]
    fn repairs_do_not_modify_original_messages_vec() {
        // Verify that validate_and_repair operates on the provided Vec and doesn't
        // mutate any external state. We pass a clone and compare.
        let original = vec![
            make_user("hello"),
            make_assistant_with_tool_use("tool-1"),
            // Deliberately missing tool_result.
        ];
        let mut api_messages = original.clone();
        let actions = validate_and_repair(&mut api_messages);

        // The original vec is untouched (we cloned it before calling repair).
        assert_eq!(original.len(), 2, "original vec must remain unmodified");
        assert_eq!(original[1].role, MessageRole::Assistant);

        // The repaired vec has the synthetic message injected.
        assert!(!actions.is_empty());
        assert_eq!(api_messages.len(), 3);
    }

    #[test]
    fn no_double_injection_when_tool_result_already_present() {
        // Both tool-1 and tool-2 used in the same assistant message; both covered.
        let mut messages = vec![
            make_user("hello"),
            ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "bash".to_string(),
                    input: "echo 1".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "bash".to_string(),
                    input: "echo 2".to_string(),
                },
            ]),
            ConversationMessage {
                role: MessageRole::Tool,
                blocks: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "tool-1".to_string(),
                        tool_name: "bash".to_string(),
                        output: "1".to_string(),
                        is_error: false,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "tool-2".to_string(),
                        tool_name: "bash".to_string(),
                        output: "2".to_string(),
                        is_error: false,
                    },
                ],
                usage: None,
            },
        ];
        let actions = validate_and_repair(&mut messages);
        assert!(actions.is_empty(), "expected no repairs: {actions:?}");
        assert_eq!(messages.len(), 3);
    }
}
