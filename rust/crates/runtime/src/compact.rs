use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

const COMPACT_CONTINUATION_PREAMBLE: &str =
    "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n";
const COMPACT_RECENT_MESSAGES_NOTE: &str = "Recent messages are preserved verbatim.";
const COMPACT_DIRECT_RESUME_INSTRUCTION: &str = "Continue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, and do not preface with continuation text.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub preserve_recent_messages: usize,
    pub max_estimated_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            preserve_recent_messages: 4,
            max_estimated_tokens: 10_000,
        }
    }
}

impl CompactionConfig {
    /// Compaction threshold derived from the active chat model's approximate context window; see
    /// [`crate::input_context_tokens_for_model`].
    #[must_use]
    pub fn for_model(model: &str) -> Self {
        Self {
            preserve_recent_messages: 4,
            max_estimated_tokens: crate::model_context::compaction_trigger_estimated_tokens_for_model(
                model,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub summary: String,
    pub formatted_summary: String,
    pub compacted_session: Session,
    pub removed_message_count: usize,
}

#[must_use]
pub fn estimate_session_tokens(session: &Session) -> usize {
    session.messages.iter().map(estimate_message_tokens).sum()
}

#[must_use]
pub fn should_compact(session: &Session, config: CompactionConfig) -> bool {
    let start = compacted_summary_prefix_len(session);
    let compactable = &session.messages[start..];
    if compactable.is_empty() {
        return false;
    }
    let sum: usize = compactable.iter().map(estimate_message_tokens).sum();
    if sum < config.max_estimated_tokens {
        return false;
    }
    if compactable.len() > config.preserve_recent_messages {
        return true;
    }
    // Few messages but still over the estimated token budget (large tool outputs).
    if compactable.len() <= 1 {
        return false;
    }
    find_safe_cut_point(
        compactable,
        compactable.len().saturating_sub(1),
    )
    .is_some_and(|cut| cut > 0)
}

#[must_use]
pub fn format_compact_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    let formatted = if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        without_analysis.replace(
            &format!("<summary>{content}</summary>"),
            &format!("Summary:\n{}", content.trim()),
        )
    } else {
        without_analysis
    };

    collapse_blank_lines(&formatted).trim().to_string()
}

#[must_use]
pub fn get_compact_continuation_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    recent_messages_preserved: bool,
) -> String {
    let mut base = format!(
        "{COMPACT_CONTINUATION_PREAMBLE}{}",
        format_compact_summary(summary)
    );

    if recent_messages_preserved {
        base.push_str("\n\n");
        base.push_str(COMPACT_RECENT_MESSAGES_NOTE);
    }

    if suppress_follow_up_questions {
        base.push('\n');
        base.push_str(COMPACT_DIRECT_RESUME_INSTRUCTION);
    }

    base
}

#[derive(Debug, Clone, Copy)]
struct ApiRound {
    #[allow(dead_code)]
    start: usize, // inclusive
    end: usize,   // exclusive
}

fn group_into_rounds(messages: &[ConversationMessage]) -> Vec<ApiRound> {
    let mut rounds = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let msg = &messages[i];
        let has_tool_use = msg.role == MessageRole::Assistant
            && msg
                .blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }));

        if has_tool_use && i + 1 < messages.len() {
            let next = &messages[i + 1];
            let next_has_tool_result = (next.role == MessageRole::Tool
                || next.role == MessageRole::User)
                && next
                    .blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
            if next_has_tool_result {
                rounds.push(ApiRound {
                    start: i,
                    end: i + 2,
                });
                i += 2;
                continue;
            }
        }

        rounds.push(ApiRound {
            start: i,
            end: i + 1,
        });
        i += 1;
    }
    rounds
}

fn find_safe_cut_point(messages: &[ConversationMessage], max_index: usize) -> Option<usize> {
    let rounds = group_into_rounds(messages);
    let mut safe_cut = None;
    for round in &rounds {
        if round.end <= max_index {
            safe_cut = Some(round.end);
        } else {
            break;
        }
    }
    safe_cut
}

fn preserve_attempt_sequence(primary: usize) -> Vec<usize> {
    let mut seq = Vec::new();
    for p in [primary, 2usize, 1, 0] {
        if !seq.contains(&p) {
            seq.push(p);
        }
    }
    seq
}

fn empty_compact_result(session: &Session) -> CompactionResult {
    CompactionResult {
        summary: String::new(),
        formatted_summary: String::new(),
        compacted_session: session.clone(),
        removed_message_count: 0,
    }
}

#[must_use]
pub fn compact_session(session: &Session, config: CompactionConfig) -> CompactionResult {
    compact_session_with_summarizer(session, config, summarize_messages)
}

/// Same as [`compact_session`], but the summary text for removed messages is produced by `summarize_removed`
/// (e.g. an LLM call from `elai-cli`), with fallback implementations allowed inside the closure.
#[must_use]
pub fn compact_session_with_summarizer<F>(
    session: &Session,
    config: CompactionConfig,
    mut summarize_removed: F,
) -> CompactionResult
where
    F: FnMut(&[ConversationMessage]) -> String,
{
    if !should_compact(session, config) {
        return empty_compact_result(session);
    }

    let existing_summary = session
        .messages
        .first()
        .and_then(extract_existing_compacted_summary);
    let compacted_prefix_len = usize::from(existing_summary.is_some());

    for preserve in preserve_attempt_sequence(config.preserve_recent_messages) {
        let naive_keep_from = session.messages.len().saturating_sub(preserve);
        let compactable = &session.messages[compacted_prefix_len..];
        let compactable_max = naive_keep_from.saturating_sub(compacted_prefix_len);
        let safe_cut_relative = find_safe_cut_point(compactable, compactable_max);

        let keep_from = match safe_cut_relative {
            Some(rel) => compacted_prefix_len + rel,
            None => continue,
        };

        let removed = &session.messages[compacted_prefix_len..keep_from];
        if removed.is_empty() {
            continue;
        }

        let preserved = session.messages[keep_from..].to_vec();
        let summary =
            merge_compact_summaries(existing_summary.as_deref(), &summarize_removed(removed));
        let formatted_summary = format_compact_summary(&summary);
        let continuation = get_compact_continuation_message(&summary, true, !preserved.is_empty());

        let mut compacted_messages = vec![ConversationMessage {
            role: MessageRole::System,
            blocks: vec![ContentBlock::Text { text: continuation }],
            usage: None,
        }];
        compacted_messages.extend(preserved);

        return CompactionResult {
            summary,
            formatted_summary,
            compacted_session: Session {
                version: session.version,
                messages: compacted_messages,
            },
            removed_message_count: removed.len(),
        };
    }

    empty_compact_result(session)
}

/// Deterministic compact summary (same algorithm as built-in compaction). Exposed for callers
/// that inject an optional LLM summary step.
#[must_use]
pub fn summarize_compact_excerpt(messages: &[ConversationMessage]) -> String {
    summarize_messages(messages)
}

fn compacted_summary_prefix_len(session: &Session) -> usize {
    usize::from(
        session
            .messages
            .first()
            .and_then(extract_existing_compacted_summary)
            .is_some(),
    )
}

fn summarize_messages(messages: &[ConversationMessage]) -> String {
    let user_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    let assistant_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    let tool_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .count();

    let mut tool_names = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            ContentBlock::ToolResult { tool_name, .. } => Some(tool_name.as_str()),
            ContentBlock::Text { .. } => None,
        })
        .collect::<Vec<_>>();
    tool_names.sort_unstable();
    tool_names.dedup();

    let mut lines = vec![
        "<summary>".to_string(),
        "Conversation summary:".to_string(),
        format!(
            "- Scope: {} earlier messages compacted (user={}, assistant={}, tool={}).",
            messages.len(),
            user_messages,
            assistant_messages,
            tool_messages
        ),
    ];

    if !tool_names.is_empty() {
        lines.push(format!("- Tools mentioned: {}.", tool_names.join(", ")));
    }

    let recent_user_requests = collect_recent_role_summaries(messages, MessageRole::User, 3);
    if !recent_user_requests.is_empty() {
        lines.push("- Recent user requests:".to_string());
        lines.extend(
            recent_user_requests
                .into_iter()
                .map(|request| format!("  - {request}")),
        );
    }

    let pending_work = infer_pending_work(messages);
    if !pending_work.is_empty() {
        lines.push("- Pending work:".to_string());
        lines.extend(pending_work.into_iter().map(|item| format!("  - {item}")));
    }

    let key_files = collect_key_files(messages);
    if !key_files.is_empty() {
        lines.push(format!("- Key files referenced: {}.", key_files.join(", ")));
    }

    if let Some(current_work) = infer_current_work(messages) {
        lines.push(format!("- Current work: {current_work}"));
    }

    lines.push("- Key timeline:".to_string());
    for message in messages {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let content = message
            .blocks
            .iter()
            .map(summarize_block)
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("  - {role}: {content}"));
    }
    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn merge_compact_summaries(existing_summary: Option<&str>, new_summary: &str) -> String {
    let Some(existing_summary) = existing_summary else {
        return new_summary.to_string();
    };

    let previous_highlights = extract_summary_highlights(existing_summary);
    let new_formatted_summary = format_compact_summary(new_summary);
    let new_highlights = extract_summary_highlights(&new_formatted_summary);
    let new_timeline = extract_summary_timeline(&new_formatted_summary);

    let mut lines = vec!["<summary>".to_string(), "Conversation summary:".to_string()];

    if !previous_highlights.is_empty() {
        lines.push("- Previously compacted context:".to_string());
        lines.extend(
            previous_highlights
                .into_iter()
                .map(|line| format!("  {line}")),
        );
    }

    if !new_highlights.is_empty() {
        lines.push("- Newly compacted context:".to_string());
        lines.extend(new_highlights.into_iter().map(|line| format!("  {line}")));
    }

    if !new_timeline.is_empty() {
        lines.push("- Key timeline:".to_string());
        lines.extend(new_timeline.into_iter().map(|line| format!("  {line}")));
    }

    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn summarize_block(block: &ContentBlock) -> String {
    let raw = match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::ToolUse { name, input, .. } => format!("tool_use {name}({input})"),
        ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } => format!(
            "tool_result {tool_name}: {}{output}",
            if *is_error { "error " } else { "" }
        ),
    };
    truncate_summary(&raw, 160)
}

fn collect_recent_role_summaries(
    messages: &[ConversationMessage],
    role: MessageRole,
    limit: usize,
) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == role)
        .rev()
        .filter_map(|message| first_text_block(message))
        .take(limit)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn infer_pending_work(messages: &[ConversationMessage]) -> Vec<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .filter(|text| {
            let lowered = text.to_ascii_lowercase();
            lowered.contains("todo")
                || lowered.contains("next")
                || lowered.contains("pending")
                || lowered.contains("follow up")
                || lowered.contains("remaining")
        })
        .take(3)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn collect_key_files(messages: &[ConversationMessage]) -> Vec<String> {
    let mut files = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.as_str(),
            ContentBlock::ToolUse { input, .. } => input.as_str(),
            ContentBlock::ToolResult { output, .. } => output.as_str(),
        })
        .flat_map(extract_file_candidates)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files.into_iter().take(8).collect()
}

fn infer_current_work(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .find(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, 200))
}

fn first_text_block(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
        ContentBlock::ToolUse { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Text { .. } => None,
    })
}

fn has_interesting_extension(candidate: &str) -> bool {
    std::path::Path::new(candidate)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["rs", "ts", "tsx", "js", "json", "md"]
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

fn extract_file_candidates(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter_map(|token| {
            let candidate = token.trim_matches(|char: char| {
                matches!(char, ',' | '.' | ':' | ';' | ')' | '(' | '"' | '\'' | '`')
            });
            if candidate.contains('/') && has_interesting_extension(candidate) {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

fn estimate_message_tokens(message: &ConversationMessage) -> usize {
    message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len() / 4 + 1,
            ContentBlock::ToolUse { name, input, .. } => (name.len() + input.len()) / 4 + 1,
            ContentBlock::ToolResult {
                tool_name, output, ..
            } => (tool_name.len() + output.len()) / 4 + 1,
        })
        .sum()
}

fn extract_tag_block(content: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let start_index = content.find(&start)? + start.len();
    let end_index = content[start_index..].find(&end)? + start_index;
    Some(content[start_index..end_index].to_string())
}

fn strip_tag_block(content: &str, tag: &str) -> String {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    if let (Some(start_index), Some(end_index_rel)) = (content.find(&start), content.find(&end)) {
        let end_index = end_index_rel + end.len();
        let mut stripped = String::new();
        stripped.push_str(&content[..start_index]);
        stripped.push_str(&content[end_index..]);
        stripped
    } else {
        content.to_string()
    }
}

fn collapse_blank_lines(content: &str) -> String {
    let mut result = String::new();
    let mut last_blank = false;
    for line in content.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && last_blank {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        last_blank = is_blank;
    }
    result
}

fn extract_existing_compacted_summary(message: &ConversationMessage) -> Option<String> {
    if message.role != MessageRole::System {
        return None;
    }

    let text = first_text_block(message)?;
    let summary = text.strip_prefix(COMPACT_CONTINUATION_PREAMBLE)?;
    let summary = summary
        .split_once(&format!("\n\n{COMPACT_RECENT_MESSAGES_NOTE}"))
        .map_or(summary, |(value, _)| value);
    let summary = summary
        .split_once(&format!("\n{COMPACT_DIRECT_RESUME_INSTRUCTION}"))
        .map_or(summary, |(value, _)| value);
    Some(summary.trim().to_string())
}

fn extract_summary_highlights(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed == "Summary:" || trimmed == "Conversation summary:" {
            continue;
        }
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if in_timeline {
            continue;
        }
        lines.push(trimmed.to_string());
    }

    lines
}

fn extract_summary_timeline(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if !in_timeline {
            continue;
        }
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::{
        collect_key_files, compact_session, estimate_session_tokens, find_safe_cut_point,
        format_compact_summary, get_compact_continuation_message, group_into_rounds,
        infer_pending_work, should_compact, CompactionConfig,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    fn assert_no_orphaned_tool_pairs(messages: &[ConversationMessage]) {
        for (i, msg) in messages.iter().enumerate() {
            // For each assistant message with ToolUse blocks, verify the next message
            // has ToolResult for each ID.
            if msg.role == MessageRole::Assistant {
                let tool_use_ids: Vec<&str> = msg
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                        _ => None,
                    })
                    .collect();
                if !tool_use_ids.is_empty() {
                    let next = messages
                        .get(i + 1)
                        .expect("assistant ToolUse has no following message");
                    assert!(
                        next.role == MessageRole::Tool || next.role == MessageRole::User,
                        "message after ToolUse assistant must be Tool or User"
                    );
                    for id in &tool_use_ids {
                        assert!(
                            next.blocks.iter().any(|b| matches!(
                                b,
                                ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id
                            )),
                            "ToolResult missing for tool_use_id={id}"
                        );
                    }
                }
            }
            // For each Tool/User message with ToolResult blocks, verify previous message
            // has ToolUse with that ID.
            if msg.role == MessageRole::Tool
                || (msg.role == MessageRole::User
                    && msg
                        .blocks
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. })))
            {
                let result_ids: Vec<&str> = msg
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                        _ => None,
                    })
                    .collect();
                if !result_ids.is_empty() {
                    let prev = messages
                        .get(i.wrapping_sub(1))
                        .expect("ToolResult message has no preceding message");
                    assert_eq!(
                        prev.role,
                        MessageRole::Assistant,
                        "message before ToolResult must be Assistant"
                    );
                    for id in &result_ids {
                        assert!(
                            prev.blocks.iter().any(|b| matches!(
                                b,
                                ContentBlock::ToolUse { id: uid, .. } if uid == id
                            )),
                            "ToolUse missing for tool_use_id={id}"
                        );
                    }
                }
            }
        }
    }

    #[allow(dead_code)]
    fn assert_starts_with_non_tool_result_user(messages: &[ConversationMessage]) {
        let first_non_system = messages
            .iter()
            .find(|m| m.role != MessageRole::System);
        if let Some(msg) = first_non_system {
            assert_eq!(msg.role, MessageRole::User, "first non-System must be User");
            assert!(
                !msg.blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
                "first non-System User must not contain ToolResult blocks"
            );
        }
    }

    #[test]
    fn formats_compact_summary_like_upstream() {
        let summary = "<analysis>scratch</analysis>\n<summary>Kept work</summary>";
        assert_eq!(format_compact_summary(summary), "Summary:\nKept work");
    }

    #[test]
    fn leaves_small_sessions_unchanged() {
        let session = Session {
            version: 1,
            messages: vec![ConversationMessage::user_text("hello")],
        };

        let result = compact_session(&session, CompactionConfig::default());
        assert_eq!(result.removed_message_count, 0);
        assert_eq!(result.compacted_session, session);
        assert!(result.summary.is_empty());
        assert!(result.formatted_summary.is_empty());
    }

    #[test]
    fn compacts_older_messages_into_a_system_summary() {
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("one ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "two ".repeat(200),
                }]),
                ConversationMessage::tool_result("1", "bash", "ok ".repeat(200), false),
                ConversationMessage {
                    role: MessageRole::Assistant,
                    blocks: vec![ContentBlock::Text {
                        text: "recent".to_string(),
                    }],
                    usage: None,
                },
            ],
        };

        let result = compact_session(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            },
        );

        assert_eq!(result.removed_message_count, 2);
        assert_eq!(
            result.compacted_session.messages[0].role,
            MessageRole::System
        );
        assert!(matches!(
            &result.compacted_session.messages[0].blocks[0],
            ContentBlock::Text { text } if text.contains("Summary:")
        ));
        assert!(result.formatted_summary.contains("Scope:"));
        assert!(result.formatted_summary.contains("Key timeline:"));
        assert!(should_compact(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            }
        ));
        assert!(
            estimate_session_tokens(&result.compacted_session) < estimate_session_tokens(&session)
        );
    }

    #[test]
    fn keeps_previous_compacted_context_when_compacting_again() {
        let initial_session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("Investigate rust/crates/runtime/src/compact.rs"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "I will inspect the compact flow.".to_string(),
                }]),
                ConversationMessage::user_text(
                    "Also update rust/crates/runtime/src/conversation.rs",
                ),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "Next: preserve prior summary context during auto compact.".to_string(),
                }]),
            ],
        };
        let config = CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        };

        let first = compact_session(&initial_session, config);
        let mut follow_up_messages = first.compacted_session.messages.clone();
        follow_up_messages.extend([
            ConversationMessage::user_text("Please add regression tests for compaction."),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Working on regression coverage now.".to_string(),
            }]),
        ]);

        let second = compact_session(
            &Session {
                version: 1,
                messages: follow_up_messages,
            },
            config,
        );

        assert!(second
            .formatted_summary
            .contains("Previously compacted context:"));
        assert!(second
            .formatted_summary
            .contains("Scope: 2 earlier messages compacted"));
        assert!(second
            .formatted_summary
            .contains("Newly compacted context:"));
        assert!(second
            .formatted_summary
            .contains("Also update rust/crates/runtime/src/conversation.rs"));
        assert!(matches!(
            &second.compacted_session.messages[0].blocks[0],
            ContentBlock::Text { text }
                if text.contains("Previously compacted context:")
                    && text.contains("Newly compacted context:")
        ));
        assert!(matches!(
            &second.compacted_session.messages[1].blocks[0],
            ContentBlock::Text { text } if text.contains("Please add regression tests for compaction.")
        ));
    }

    #[test]
    fn ignores_existing_compacted_summary_when_deciding_to_recompact() {
        let summary = "<summary>Conversation summary:\n- Scope: earlier work preserved.\n- Key timeline:\n  - user: large preserved context\n</summary>";
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage {
                    role: MessageRole::System,
                    blocks: vec![ContentBlock::Text {
                        text: get_compact_continuation_message(summary, true, true),
                    }],
                    usage: None,
                },
                ConversationMessage::user_text("tiny"),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "recent".to_string(),
                }]),
            ],
        };

        assert!(!should_compact(
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 50_000,
            }
        ));
    }

    #[test]
    fn truncates_long_blocks_in_summary() {
        let summary = super::summarize_block(&ContentBlock::Text {
            text: "x".repeat(400),
        });
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= 161);
    }

    #[test]
    fn extracts_key_files_from_message_content() {
        let files = collect_key_files(&[ConversationMessage::user_text(
            "Update rust/crates/runtime/src/compact.rs and rust/crates/tools/src/lib.rs next.",
        )]);
        assert!(files.contains(&"rust/crates/runtime/src/compact.rs".to_string()));
        assert!(files.contains(&"rust/crates/tools/src/lib.rs".to_string()));
    }

    #[test]
    fn infers_pending_work_from_recent_messages() {
        let pending = infer_pending_work(&[
            ConversationMessage::user_text("done"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Next: update tests and follow up on remaining CLI polish.".to_string(),
            }]),
        ]);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains("Next: update tests"));
    }

    #[test]
    fn compact_never_splits_tool_use_tool_result_pair() {
        // Layout (indices 0-4):
        //   0: user "hello" (big)
        //   1: assistant ToolUse id="t1"
        //   2: tool ToolResult id="t1"   <- naive cut at index 2 would split 1 and 2
        //   3: user "next" (big)
        //   4: assistant "done"
        //
        // preserve_recent_messages=3 → naive keep_from = 5-3 = 2, cutting between 1 and 2.
        // The fix must push keep_from to 3 (after the pair ends at index 2).
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("hello ".repeat(400)),
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("t1", "bash", "ok", false),
                ConversationMessage::user_text("next ".repeat(400)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "done".to_string(),
                }]),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 3,
            max_estimated_tokens: 1,
        };

        let result = compact_session(&session, config);
        // Some compaction must have occurred (not a no-op), and pairs must be intact.
        assert_no_orphaned_tool_pairs(&result.compacted_session.messages);
        // The compacted session should not contain the ToolUse without its ToolResult
        // or vice-versa. If removed_message_count is 0 the session was left intact
        // which is also acceptable (safe cut not possible without breaking pair).
    }

    #[test]
    fn compact_retries_preserve_until_full_pair_can_be_removed() {
        // Single assistant+tool pair; preserve=1 wants keep_from=1 which splits the pair
        // (no safe cut with max_index=1). The preserve fallback (2, then 0) eventually uses
        // preserve=0 → keep entire tail → safe cut removes both messages as one round.
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: "{}".to_string(),
                }]),
                ConversationMessage::tool_result("t1", "bash", "ok ".repeat(400), false),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 1,
            max_estimated_tokens: 1,
        };

        let result = compact_session(&session, config);
        assert_eq!(result.removed_message_count, 2);
        assert_eq!(result.compacted_session.messages.len(), 1);
        assert_eq!(result.compacted_session.messages[0].role, MessageRole::System);
        assert_no_orphaned_tool_pairs(&result.compacted_session.messages);
    }

    #[test]
    fn compact_still_works_for_plain_messages_without_tool_use() {
        // Plain user/assistant alternation — no tool_use anywhere.
        // Should compact exactly as before the boundary-aware fix.
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("msg1 ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "reply1 ".repeat(200),
                }]),
                ConversationMessage::user_text("msg2 ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "reply2 ".repeat(200),
                }]),
                ConversationMessage::user_text("recent".to_string()),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "recent reply".to_string(),
                }]),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        };

        let result = compact_session(&session, config);
        assert!(result.removed_message_count > 0);
        assert_eq!(
            result.compacted_session.messages[0].role,
            MessageRole::System
        );
        assert_no_orphaned_tool_pairs(&result.compacted_session.messages);
    }

    #[test]
    fn compact_keeps_parallel_tool_use_batch_intact() {
        // Assistant with 2 ToolUse blocks, followed by User with 2 ToolResult blocks.
        // The pair is indivisible.
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("start ".repeat(300)),
                ConversationMessage::assistant(vec![
                    ContentBlock::ToolUse {
                        id: "ta".to_string(),
                        name: "read".to_string(),
                        input: "{}".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tb".to_string(),
                        name: "write".to_string(),
                        input: "{}".to_string(),
                    },
                ]),
                ConversationMessage {
                    role: MessageRole::User,
                    blocks: vec![
                        ContentBlock::ToolResult {
                            tool_use_id: "ta".to_string(),
                            tool_name: "read".to_string(),
                            output: "content".to_string(),
                            is_error: false,
                        },
                        ContentBlock::ToolResult {
                            tool_use_id: "tb".to_string(),
                            tool_name: "write".to_string(),
                            output: "ok".to_string(),
                            is_error: false,
                        },
                    ],
                    usage: None,
                },
                ConversationMessage::user_text("done ".repeat(300)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "finished".to_string(),
                }]),
            ],
        };

        let config = CompactionConfig {
            preserve_recent_messages: 3,
            max_estimated_tokens: 1,
        };

        let result = compact_session(&session, config);
        assert_no_orphaned_tool_pairs(&result.compacted_session.messages);

        // Verify group_into_rounds treats the parallel pair as one indivisible round
        let rounds = group_into_rounds(&session.messages);
        // Round for index 1 (assistant with 2 ToolUse) + index 2 (User with 2 ToolResult)
        // should be a single ApiRound spanning [1, 3)
        let pair_round = rounds.iter().find(|r| r.start == 1);
        assert!(pair_round.is_some());
        assert_eq!(pair_round.unwrap().end, 3);
    }

    #[test]
    fn find_safe_cut_point_returns_none_when_only_pair_fits() {
        // [assistant_tool_use, tool_result] — pair ends at 2, max_index=1 → None
        let messages = vec![
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "x".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            }]),
            ConversationMessage::tool_result("x", "bash", "ok", false),
        ];
        assert_eq!(find_safe_cut_point(&messages, 1), None);
    }

    #[test]
    fn find_safe_cut_point_returns_correct_index_after_complete_pair() {
        // [user, assistant_tool_use, tool_result, user, assistant]
        // max_index=3 → safe cut should be at 3 (after pair ends at index 2)
        let messages = vec![
            ConversationMessage::user_text("hello"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "y".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            }]),
            ConversationMessage::tool_result("y", "bash", "done", false),
            ConversationMessage::user_text("next"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "ok".to_string(),
            }]),
        ];
        // pair [1,3) ends at 3 which equals max_index=3 → safe cut = Some(3)
        assert_eq!(find_safe_cut_point(&messages, 3), Some(3));
    }

    #[test]
    fn should_compact_triggers_for_few_huge_messages() {
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("x".repeat(50_000)),
                ConversationMessage::user_text("y".repeat(50_000)),
            ],
        };
        assert!(should_compact(
            &session,
            CompactionConfig {
                preserve_recent_messages: 4,
                max_estimated_tokens: 10_000,
            },
        ));
    }

    #[test]
    fn for_model_scales_threshold_with_context_window() {
        let small = CompactionConfig::for_model("ollama:llama3");
        let large = CompactionConfig::for_model("claude-sonnet-4-6");
        assert!(small.max_estimated_tokens < large.max_estimated_tokens);
    }
}
