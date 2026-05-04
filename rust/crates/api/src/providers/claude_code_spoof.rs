//! Claude Code OAuth bypass / spoof module.
//!
//! Ports `anthropic_billing_bypass.py` v1.1.1 from
//! <https://github.com/kristianvast/hermes-claude-auth>.
//!
//! Transforms outgoing request bodies and incoming response bodies so that
//! OAuth-authenticated requests pass Anthropic's server-side content
//! validation and continue to route to the Claude Max/Pro subscription tier.

use sha2::{Digest, Sha256};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Inline hex helper (avoids an external `hex` crate dep)
// ---------------------------------------------------------------------------
fn to_hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BILLING_SALT: &str = "59cf53e54c78";
const BILLING_ENTRYPOINT: &str = "sdk-cli";
const BILLING_PREFIX: &str = "x-anthropic-billing-header";
const SYSTEM_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const MCP_PREFIX: &str = "mcp_";
const STAINLESS_PACKAGE_VERSION: &str = "0.81.0";
const STAINLESS_NODE_VERSION: &str = "v22.11.0";
pub const CLAUDE_CODE_VERSION_FALLBACK: &str = "2.1.112";

const EXTRA_OAUTH_BETAS_LIST: &[&str] = &[
    "claude-code-20250219",
    "oauth-2025-04-20",
    "prompt-caching-scope-2026-01-05",
    "advisor-tool-2026-03-01",
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Orchestrates all request transforms. Returns the list of `PascalCased` tool
/// names that must later be reversed in the response.
///
/// Order:
/// 1. `inject_billing_header` (uses original messages for SHA-256 — must run first)
/// 2. prepend moved system texts to first user message
/// 3. `pascalcase_mcp_tool_names` → collects modified names
/// 4. `fix_temperature_for_adaptive_models`
pub fn apply_request_transform(body: &mut Value, cli_version: &str) -> Vec<String> {
    let moved_texts = inject_billing_header(body, cli_version);
    prepend_to_first_user_message(body, &moved_texts);
    let modified = pascalcase_mcp_tool_names(body);
    fix_temperature_for_adaptive_models(body);
    modified
}

/// Reverse the `PascalCase` rewrite on tool-use blocks in a response body.
pub fn apply_response_transform(body: &mut Value, modified_tool_names: &[String]) {
    if modified_tool_names.is_empty() {
        return;
    }
    let Some(content) = body.get_mut("content").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for block in content.iter_mut() {
        reverse_pascalcase_in_block(block, modified_tool_names);
    }
}

/// Returns the nine Stainless/browser-access spoofing headers that real
/// Claude Code 2.1.112 sends.
#[must_use] 
pub fn stainless_headers(_cli_version: &str) -> Vec<(&'static str, String)> {
    vec![
        ("anthropic-dangerous-direct-browser-access", "true".into()),
        ("x-stainless-arch", stainless_arch().into()),
        ("x-stainless-lang", "js".into()),
        ("x-stainless-os", stainless_os().into()),
        ("x-stainless-package-version", STAINLESS_PACKAGE_VERSION.into()),
        ("x-stainless-retry-count", "0".into()),
        ("x-stainless-runtime", "node".into()),
        ("x-stainless-runtime-version", STAINLESS_NODE_VERSION.into()),
        ("x-stainless-timeout", "600".into()),
    ]
}

/// Beta flags that OAuth requests need in addition to the base set.
#[must_use] 
pub fn extra_oauth_betas() -> &'static [&'static str] {
    EXTRA_OAUTH_BETAS_LIST
}

/// Relocate moved system texts (produced by `inject_billing_header`) to the
/// first user message as `<system-reminder>` blocks.
///
/// This is the public wrapper around `prepend_to_first_user_message` for
/// callers that want fine-grained control.
pub fn relocate_system_to_user_messages(body: &mut Value) {
    // When called standalone, there are no moved texts yet — this is a no-op.
    // The real relocation is driven by `apply_request_transform`, which passes
    // the Vec returned by `inject_billing_header`.
    let _ = body;
}

/// Inject the cryptographically-signed billing header into `body["system"]`.
///
/// Returns the list of text strings that were displaced from `system` and
/// should be prepended to the first user message as `<system-reminder>` blocks.
pub fn inject_billing_header(body: &mut Value, cli_version: &str) -> Vec<String> {
    // Compute billing header using ORIGINAL messages (before any relocation).
    let billing_value = {
        let messages = body.get("messages").cloned().unwrap_or(Value::Null);
        build_billing_header_value(&messages, cli_version, BILLING_ENTRYPOINT)
    };
    let billing_entry = serde_json::json!({"type": "text", "text": billing_value});

    // Normalise system to a Vec of block objects.
    let raw_system = body.get("system").cloned().unwrap_or(Value::Null);
    let system_blocks: Vec<Value> = match raw_system {
        Value::Null => vec![],
        Value::String(s) => {
            if s.is_empty() {
                vec![]
            } else {
                vec![serde_json::json!({"type": "text", "text": s})]
            }
        }
        Value::Array(arr) => arr,
        other => {
            eprintln!(
                "[claude_code_spoof] inject_billing_header: unexpected system type {other:?}; skipping"
            );
            return vec![];
        }
    };

    let mut kept: Vec<Value> = Vec::new();
    let mut moved_texts: Vec<String> = Vec::new();
    let mut identity_seen = false;

    for entry in system_blocks {
        let Some(obj) = entry.as_object() else {
            // Non-object entry — keep as-is.
            kept.push(entry);
            continue;
        };
        let entry_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if entry_type != "text" {
            // Non-text block (e.g. image) — keep as-is.
            kept.push(Value::Object(obj.clone()));
            continue;
        }
        let text = obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if text.starts_with(BILLING_PREFIX) {
            // Stale billing header — drop.
            continue;
        }
        if let Some(stripped) = text.strip_prefix(SYSTEM_IDENTITY) {
            if identity_seen {
                // Duplicate identity entry — drop.
                continue;
            }
            identity_seen = true;
            // Truncate text to exactly SYSTEM_IDENTITY; move the rest.
            let rest = stripped.trim_start_matches('\n').to_string();
            let mut identity_obj = obj.clone();
            identity_obj.insert("text".to_string(), Value::String(SYSTEM_IDENTITY.to_string()));
            kept.push(Value::Object(identity_obj));
            if !rest.is_empty() {
                moved_texts.push(rest);
            }
            continue;
        }
        // Any other non-empty text block → relocate.
        if !text.is_empty() {
            moved_texts.push(text.to_string());
        }
        // Empty text blocks are silently dropped.
    }

    if !identity_seen {
        kept.insert(0, serde_json::json!({"type": "text", "text": SYSTEM_IDENTITY}));
    }

    // Billing header is always first.
    let mut new_system = vec![billing_entry];
    new_system.extend(kept);
    body["system"] = Value::Array(new_system);

    moved_texts
}

/// Rewrite `mcp_foo` → `mcp_Foo` in tool definitions and tool-use blocks
/// inside messages. Returns the list of rewritten names (for later reversal).
pub fn pascalcase_mcp_tool_names(body: &mut Value) -> Vec<String> {
    let mut modified: Vec<String> = Vec::new();

    // Rewrite tool definitions.
    if let Some(tools) = body.get_mut("tools").and_then(|v| v.as_array_mut()) {
        for tool in tools.iter_mut() {
            if let Some(name) = tool.get_mut("name") {
                if let Some(s) = name.as_str() {
                    if let Some(pascal) = try_pascalcase_mcp(s) {
                        if !modified.contains(&pascal) {
                            modified.push(pascal.clone());
                        }
                        *name = Value::String(pascal);
                    }
                }
            }
        }
    }

    // Rewrite tool_use blocks in messages.
    if let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) {
        for msg in messages.iter_mut() {
            if let Some(content) = msg.get_mut("content").and_then(|v| v.as_array_mut()) {
                for block in content.iter_mut() {
                    let is_tool_use = block
                        .get("type")
                        .and_then(|v| v.as_str()) == Some("tool_use");
                    if is_tool_use {
                        if let Some(name) = block.get_mut("name") {
                            if let Some(s) = name.as_str() {
                                if let Some(pascal) = try_pascalcase_mcp(s) {
                                    if !modified.contains(&pascal) {
                                        modified.push(pascal.clone());
                                    }
                                    *name = Value::String(pascal);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    modified
}

/// Drop `temperature` from the body when the model supports adaptive thinking
/// and temperature is not already 1.
pub fn fix_temperature_for_adaptive_models(body: &mut Value) {
    let Some(temp) = body.get("temperature") else {
        return;
    };
    // Keep temperature == 1 or 1.0 unchanged.
    let is_one = temp.as_f64().is_some_and(|t| (t - 1.0_f64).abs() < f64::EPSILON);
    if is_one {
        return;
    }
    let model_is_adaptive = body
        .get("model")
        .and_then(|v| v.as_str())
        .is_some_and(|m| m.contains("4-6") || m.contains("4.6"));
    if model_is_adaptive {
        body.as_object_mut().map(|o| o.remove("temperature"));
    }
}

/// Reverse a `PascalCased` MCP tool name in a streaming SSE event value.
///
/// Handles `content_block_start` events that carry a `content_block` with
/// `type == "tool_use"`.
pub fn reverse_pascalcase_mcp_in_streaming_event(
    event_value: &mut Value,
    modified: &[String],
) {
    if modified.is_empty() {
        return;
    }
    let event_type = event_value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if event_type == "content_block_start" {
        if let Some(block) = event_value.get_mut("content_block") {
            reverse_pascalcase_in_block(block, modified);
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the text of the first user message's first text block.
pub(crate) fn extract_first_user_message_text(messages: &Value) -> String {
    let Some(arr) = messages.as_array() else {
        return String::new();
    };
    for msg in arr {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" {
            continue;
        }
        let content = msg.get("content");
        match content {
            Some(Value::String(s)) => return s.clone(),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                        let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            return text.to_string();
                        }
                    }
                }
                // Found a user message but no text block.
                return String::new();
            }
            _ => return String::new(),
        }
    }
    String::new()
}

/// First 5 hex characters of SHA-256(text).
pub(crate) fn compute_cch(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    to_hex_lower(&result)[..5].to_string()
}

/// 3-char version suffix derived from sampled chars at positions 4, 7, 20.
pub(crate) fn compute_version_suffix(text: &str, version: &str) -> String {
    let char_at = |idx: usize| -> char { text.chars().nth(idx).unwrap_or('0') };
    let sampled: String = [char_at(4), char_at(7), char_at(20)].iter().collect();
    let input = format!("{BILLING_SALT}{sampled}{version}");
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    to_hex_lower(&result)[..3].to_string()
}

/// Full billing header value string.
pub(crate) fn build_billing_header_value(
    messages: &Value,
    version: &str,
    entrypoint: &str,
) -> String {
    let text = extract_first_user_message_text(messages);
    let suffix = compute_version_suffix(&text, version);
    let cch = compute_cch(&text);
    format!(
        "x-anthropic-billing-header: cc_version={version}.{suffix}; cc_entrypoint={entrypoint}; cch={cch};"
    )
}

/// Return `mcp_Foo` if `name` is `mcp_foo` (first char after prefix is lowercase).
/// Returns `None` when no rewrite is needed.
fn try_pascalcase_mcp(name: &str) -> Option<String> {
    if !name.starts_with(MCP_PREFIX) {
        return None;
    }
    let rest = &name[MCP_PREFIX.len()..];
    let first = rest.chars().next()?;
    if !first.is_lowercase() {
        return None;
    }
    let mut new_name = MCP_PREFIX.to_string();
    new_name.push(first.to_ascii_uppercase());
    new_name.push_str(&rest[first.len_utf8()..]);
    Some(new_name)
}

/// Reverse a `PascalCased` MCP tool name on a single JSON block in place.
fn reverse_pascalcase_in_block(block: &mut Value, modified: &[String]) {
    let is_tool_use = block
        .get("type")
        .and_then(|v| v.as_str()) == Some("tool_use");
    if !is_tool_use {
        return;
    }
    let name_matches = block
        .get("name")
        .and_then(|v| v.as_str())
        .is_some_and(|n| modified.contains(&n.to_string()));
    if !name_matches {
        return;
    }
    if let Some(name_val) = block.get_mut("name") {
        if let Some(s) = name_val.as_str() {
            if let Some(rest) = s.strip_prefix(MCP_PREFIX) {
                if let Some(first) = rest.chars().next() {
                    if first.is_uppercase() {
                        let mut lowered = MCP_PREFIX.to_string();
                        lowered.push(first.to_ascii_lowercase());
                        lowered.push_str(&rest[first.len_utf8()..]);
                        *name_val = Value::String(lowered);
                    }
                }
            }
        }
    }
}

/// Prepend `moved_texts` as `<system-reminder>` blocks to the first user message.
fn prepend_to_first_user_message(body: &mut Value, texts: &[String]) {
    if texts.is_empty() {
        return;
    }
    let combined = texts
        .iter()
        .map(|t| format!("<system-reminder>\n{t}\n</system-reminder>"))
        .collect::<Vec<_>>()
        .join("\n\n");

    let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };

    for msg in messages.iter_mut() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" {
            continue;
        }
        let content = msg.get("content").cloned();
        match content {
            Some(Value::String(s)) => {
                let new_text = if s.is_empty() {
                    combined.clone()
                } else {
                    format!("{combined}\n\n{s}")
                };
                msg["content"] = Value::Array(vec![
                    serde_json::json!({"type": "text", "text": new_text}),
                ]);
            }
            Some(Value::Array(mut blocks)) => {
                // Find first text block and prepend.
                let mut found = false;
                for block in &mut blocks {
                    if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                        let existing = block
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let new_text = if existing.is_empty() {
                            combined.clone()
                        } else {
                            format!("{combined}\n\n{existing}")
                        };
                        block["text"] = Value::String(new_text);
                        found = true;
                        break;
                    }
                }
                if !found {
                    blocks.insert(0, serde_json::json!({"type": "text", "text": combined}));
                }
                msg["content"] = Value::Array(blocks);
            }
            _ => {
                msg["content"] = Value::Array(vec![
                    serde_json::json!({"type": "text", "text": combined}),
                ]);
            }
        }
        return; // Only modify the first user message.
    }
}

fn stainless_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else {
        "unknown"
    }
}

fn stainless_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "MacOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Unknown"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Helper: SHA-256 hex of a string.
    fn sha256_hex(s: &str) -> String {
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        to_hex_lower(&h.finalize())
    }

    // 1. Known vector for compute_cch.
    #[test]
    fn cch_matches_known_vector() {
        let text = "hello world";
        let full_hex = sha256_hex(text);
        let expected = &full_hex[..5];
        assert_eq!(compute_cch(text), expected);
    }

    // 2. Short message → indices 4/7/20 all yield '0'.
    #[test]
    fn version_suffix_for_short_message_pads_with_zero() {
        let text = "abc"; // len 3; chars at 4, 7, 20 are all missing → '0'
        let sampled = "000";
        let version = "2.1.112";
        let input = format!("{BILLING_SALT}{sampled}{version}");
        let expected_full = sha256_hex(&input);
        let expected = &expected_full[..3];
        assert_eq!(compute_version_suffix(text, version), expected);
    }

    // 3. Billing header format.
    #[test]
    fn billing_header_format_round_trip() {
        let messages = json!([{"role": "user", "content": "Hello!"}]);
        let version = "2.1.112";
        let header = build_billing_header_value(&messages, version, BILLING_ENTRYPOINT);
        assert!(header.starts_with("x-anthropic-billing-header: cc_version=2.1.112."));
        assert!(header.contains("; cc_entrypoint=sdk-cli;"));
        assert!(header.contains("; cch="));
        assert!(header.ends_with(';'));
    }

    // 4. null system → [billing, identity].
    #[test]
    fn inject_billing_header_with_empty_system_creates_array_with_billing_then_identity() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "hi"}],
            "system": null
        });
        let moved = inject_billing_header(&mut body, "2.1.112");
        assert!(moved.is_empty());
        let system = body["system"].as_array().unwrap();
        assert_eq!(system.len(), 2);
        assert!(system[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("x-anthropic-billing-header:"));
        assert_eq!(system[1]["text"].as_str().unwrap(), SYSTEM_IDENTITY);
    }

    // 5. Stale billing header is replaced.
    #[test]
    fn inject_billing_header_drops_stale_billing() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "hi"}],
            "system": [{"type": "text", "text": "x-anthropic-billing-header: old stuff"}]
        });
        inject_billing_header(&mut body, "2.1.112");
        let system = body["system"].as_array().unwrap();
        // Old entry must be gone; the new one is system[0].
        assert!(!system
            .iter()
            .any(|e| e["text"].as_str().unwrap_or("").contains("old stuff")));
        assert!(system[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("x-anthropic-billing-header:"));
    }

    // 6. Extra text after SYSTEM_IDENTITY is moved.
    #[test]
    fn inject_billing_header_extracts_extra_text_from_identity_entry() {
        let full_text = format!("{SYSTEM_IDENTITY}\n\nadditional context");
        let mut body = json!({
            "messages": [{"role": "user", "content": "hi"}],
            "system": [{"type": "text", "text": full_text}]
        });
        let moved = inject_billing_header(&mut body, "2.1.112");
        assert!(moved.iter().any(|t| t.contains("additional context")));
        let system = body["system"].as_array().unwrap();
        let identity_entry = system.iter().find(|e| {
            e["text"].as_str().unwrap_or("").starts_with(SYSTEM_IDENTITY)
                && !e["text"].as_str().unwrap_or("").starts_with("x-anthropic-billing-header")
        });
        assert!(identity_entry.is_some());
        assert_eq!(
            identity_entry.unwrap()["text"].as_str().unwrap(),
            SYSTEM_IDENTITY
        );
    }

    // 7. Relocation wraps texts in <system-reminder> blocks.
    #[test]
    fn relocate_creates_system_reminder_blocks_in_first_user_message() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "original"}],
            "system": null
        });
        let texts = vec!["ctx A".to_string(), "ctx B".to_string()];
        prepend_to_first_user_message(&mut body, &texts);
        let content = &body["messages"][0]["content"];
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("<system-reminder>\nctx A\n</system-reminder>"));
        assert!(text.contains("<system-reminder>\nctx B\n</system-reminder>"));
        assert!(text.ends_with("original"));
    }

    // 8. Only lowercase mcp_ names are rewritten.
    #[test]
    fn pascalcase_rewrites_only_mcp_lowercase_tool_names() {
        let mut body = json!({
            "messages": [],
            "tools": [
                {"name": "mcp_bash"},
                {"name": "mcp_Read"},
                {"name": "web_search"}
            ]
        });
        let modified = pascalcase_mcp_tool_names(&mut body);
        assert_eq!(body["tools"][0]["name"].as_str().unwrap(), "mcp_Bash");
        assert_eq!(body["tools"][1]["name"].as_str().unwrap(), "mcp_Read"); // already PascalCase
        assert_eq!(body["tools"][2]["name"].as_str().unwrap(), "web_search"); // no mcp_ prefix
        assert_eq!(modified, vec!["mcp_Bash"]);
    }

    // 9. Tool-use blocks inside messages are also rewritten.
    #[test]
    fn pascalcase_rewrites_tool_use_blocks_in_messages() {
        let mut body = json!({
            "tools": [],
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "id": "abc", "name": "mcp_write", "input": {}}
                    ]
                }
            ]
        });
        let modified = pascalcase_mcp_tool_names(&mut body);
        assert_eq!(
            body["messages"][0]["content"][0]["name"].as_str().unwrap(),
            "mcp_Write"
        );
        assert!(modified.contains(&"mcp_Write".to_string()));
    }

    // 10. Temperature dropped for adaptive model.
    #[test]
    fn fix_temperature_drops_when_adaptive_and_not_one() {
        let mut body = json!({
            "model": "claude-opus-4-6",
            "temperature": 0.5
        });
        fix_temperature_for_adaptive_models(&mut body);
        assert!(body.get("temperature").is_none());
    }

    // 11. Temperature == 1 is kept.
    #[test]
    fn fix_temperature_keeps_when_temperature_is_one() {
        let mut body = json!({
            "model": "claude-opus-4-6",
            "temperature": 1
        });
        fix_temperature_for_adaptive_models(&mut body);
        assert!(body.get("temperature").is_some());
    }

    // 12. Temperature kept when model is not adaptive.
    #[test]
    fn fix_temperature_keeps_when_model_not_adaptive() {
        let mut body = json!({
            "model": "claude-haiku-4-5",
            "temperature": 0.5
        });
        fix_temperature_for_adaptive_models(&mut body);
        assert_eq!(body["temperature"].as_f64().unwrap(), 0.5);
    }

    // 13. Response transform reverses PascalCase.
    #[test]
    fn apply_response_transform_reverses_pascalcase_in_tool_use() {
        let mut body = json!({
            "content": [
                {"type": "tool_use", "id": "x", "name": "mcp_Bash", "input": {}}
            ]
        });
        apply_response_transform(&mut body, &["mcp_Bash".to_string()]);
        assert_eq!(body["content"][0]["name"].as_str().unwrap(), "mcp_bash");
    }

    // 14. Streaming event reversal.
    #[test]
    fn reverse_in_streaming_event_unwraps_content_block_start() {
        let mut event = json!({
            "type": "content_block_start",
            "content_block": {"type": "tool_use", "id": "y", "name": "mcp_Bash"}
        });
        reverse_pascalcase_mcp_in_streaming_event(&mut event, &["mcp_Bash".to_string()]);
        assert_eq!(
            event["content_block"]["name"].as_str().unwrap(),
            "mcp_bash"
        );
    }

    // 15. stainless_headers returns all nine required keys.
    #[test]
    fn stainless_headers_includes_all_nine_required_keys() {
        let headers = stainless_headers("2.1.112");
        let keys: Vec<&str> = headers.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"anthropic-dangerous-direct-browser-access"));
        assert!(keys.contains(&"x-stainless-arch"));
        assert!(keys.contains(&"x-stainless-lang"));
        assert!(keys.contains(&"x-stainless-os"));
        assert!(keys.contains(&"x-stainless-package-version"));
        assert!(keys.contains(&"x-stainless-retry-count"));
        assert!(keys.contains(&"x-stainless-runtime"));
        assert!(keys.contains(&"x-stainless-runtime-version"));
        assert!(keys.contains(&"x-stainless-timeout"));
        assert_eq!(headers.len(), 9);
    }

    // 16. extra_oauth_betas includes required entries.
    #[test]
    fn extra_oauth_betas_includes_claude_code_and_advisor_tool() {
        let betas = extra_oauth_betas();
        assert!(betas.contains(&"claude-code-20250219"));
        assert!(betas.contains(&"advisor-tool-2026-03-01"));
        assert!(betas.contains(&"prompt-caching-scope-2026-01-05"));
        assert!(betas.contains(&"oauth-2025-04-20"));
    }
}
