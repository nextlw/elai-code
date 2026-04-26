//! Strict Write Discipline (SWD) — transactional filesystem write engine.
//!
//! This module is intentionally pure: it does not depend on the TUI or runtime
//! crates. It exposes the level enum, snapshot/rollback helpers, the
//! `[FILE_ACTION]` parser/executor for full mode, and a JSON-lines logger.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

// ─── Level ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SwdLevel {
    Off = 0,
    #[default]
    Partial = 1,
    Full = 2,
}

impl SwdLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Off,
            2 => Self::Full,
            _ => Self::Partial,
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "partial" => Some(Self::Partial),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Partial => "partial",
            Self::Full => "full",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Partial,
            Self::Partial => Self::Full,
            Self::Full => Self::Off,
        }
    }
}

// ─── Outcome ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SwdOutcome {
    Verified,
    Noop,
    Drift {
        #[allow(dead_code)]
        detail: String,
    },
    Failed {
        #[allow(dead_code)]
        reason: String,
    },
    RolledBack,
}

impl SwdOutcome {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Verified => "Verified",
            Self::Noop => "Noop",
            Self::Drift { .. } => "Drift",
            Self::Failed { .. } => "Failed",
            Self::RolledBack => "RolledBack",
        }
    }
}

// ─── Transaction ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SwdTransaction {
    pub tool_name: String,
    pub path: String,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub outcome: SwdOutcome,
    pub timestamp_ms: u64,
}

// ─── Correction Context ──────────────────────────────────────

pub const MAX_CORRECTION_ATTEMPTS: u8 = 2;

#[derive(Debug, Clone)]
pub struct CorrectionContext {
    pub attempts: u8,
    pub max_attempts: u8,
    pub last_failures: Vec<SwdTransaction>,
}

impl CorrectionContext {
    pub fn new() -> Self {
        Self {
            attempts: 0,
            max_attempts: MAX_CORRECTION_ATTEMPTS,
            last_failures: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
        self.last_failures.clear();
    }

    pub fn can_retry(&self) -> bool {
        self.attempts < self.max_attempts
    }

    pub fn record_failures(&mut self, txs: &[SwdTransaction]) {
        self.attempts += 1;
        self.last_failures = txs
            .iter()
            .filter(|tx| {
                matches!(
                    tx.outcome,
                    SwdOutcome::Failed { .. } | SwdOutcome::Drift { .. } | SwdOutcome::RolledBack
                )
            })
            .cloned()
            .collect();
    }

    pub fn has_failures(&self) -> bool {
        !self.last_failures.is_empty()
    }
}

impl Default for CorrectionContext {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
pub fn build_correction_prompt(failures: &[SwdTransaction]) -> String {
    let mut lines = vec!["[SWD CORRECTION TURN]".to_string()];
    lines.push("File actions failed verification:".to_string());
    for tx in failures {
        let status = tx.outcome.as_str().to_uppercase();
        let detail = match &tx.outcome {
            SwdOutcome::Failed { reason } => reason.clone(),
            SwdOutcome::Drift { detail } => detail.clone(),
            SwdOutcome::RolledBack => "rolled back after batch failure".to_string(),
            _ => String::new(),
        };
        lines.push(format!(
            "- [{status}] {tool} {path}: {detail} (before={before}, after={after})",
            tool = tx.tool_name,
            path = tx.path,
            before = tx.before_hash.as_deref().unwrap_or("none"),
            after = tx.after_hash.as_deref().unwrap_or("none"),
        ));
    }
    lines.push(String::new());
    lines.push("Please correct your response and retry the failed file operations.".to_string());
    lines.join("\n")
}

// ─── Hashing ─────────────────────────────────────────────────

pub fn hash_content(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

// ─── Snapshot ────────────────────────────────────────────────

/// Returns `(sha256_hex, raw_bytes)` or `(None, None)` if the file does not exist.
pub fn snapshot(path: &str) -> (Option<String>, Option<Vec<u8>>) {
    match std::fs::read(path) {
        Ok(bytes) => {
            let hash = hash_content(&bytes);
            (Some(hash), Some(bytes))
        }
        Err(_) => (None, None),
    }
}

// ─── Rollback ────────────────────────────────────────────────

/// Restores a file to its before-state. If `before` is `None`, deletes the file.
pub fn rollback(path: &str, before: Option<&[u8]>) -> io::Result<()> {
    match before {
        Some(content) => {
            if let Some(parent) = Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(path, content)
        }
        None => {
            if Path::new(path).exists() {
                std::fs::remove_file(path)?;
            }
            Ok(())
        }
    }
}

// ─── Verify ──────────────────────────────────────────────────

pub fn verify_outcome(
    before_hash: &Option<String>,
    after_hash: &Option<String>,
    tool_ok: bool,
) -> SwdOutcome {
    if !tool_ok {
        return SwdOutcome::Failed {
            reason: "tool execution returned error".to_string(),
        };
    }
    match (before_hash, after_hash) {
        (b, a) if b == a => SwdOutcome::Noop,
        (_, Some(_)) => SwdOutcome::Verified,
        (Some(_), None) => SwdOutcome::Verified, // deletion
        (None, None) => SwdOutcome::Noop,
    }
}

// ─── Full mode: FileAction ────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Write,
    Delete,
}

#[derive(Debug, Clone)]
pub struct FileAction {
    pub path: String,
    pub operation: FileOp,
    pub content: Option<String>,
    pub content_hash: Option<String>,
}

/// Parses `[FILE_ACTION:Write/Delete]` blocks from model text output.
pub fn parse_file_actions(text: &str) -> Vec<FileAction> {
    let mut actions = Vec::new();
    let mut cursor = 0;
    let start_write = "[FILE_ACTION:Write]";
    let start_delete = "[FILE_ACTION:Delete]";
    let end_tag = "[/FILE_ACTION]";

    while cursor < text.len() {
        let (op, start_idx) = {
            let wi = text[cursor..]
                .find(start_write)
                .map(|i| (FileOp::Write, cursor + i));
            let di = text[cursor..]
                .find(start_delete)
                .map(|i| (FileOp::Delete, cursor + i));
            match (wi, di) {
                (Some(w), Some(d)) => {
                    if w.1 <= d.1 {
                        w
                    } else {
                        d
                    }
                }
                (Some(w), None) => w,
                (None, Some(d)) => d,
                (None, None) => break,
            }
        };

        let tag_len = match op {
            FileOp::Write => start_write.len(),
            FileOp::Delete => start_delete.len(),
        };
        let block_start = start_idx + tag_len;

        let end_idx = match text[block_start..].find(end_tag) {
            Some(i) => block_start + i,
            None => {
                cursor = start_idx + 1;
                continue;
            }
        };

        let block = &text[block_start..end_idx];
        cursor = end_idx + end_tag.len();

        let mut path = String::new();
        let mut content_hash: Option<String> = None;
        let mut content: Option<String> = None;

        for line in block.lines() {
            let line = line.trim();
            if let Some(p) = line.strip_prefix("path:") {
                path = p.trim().to_string();
            } else if let Some(h) = line.strip_prefix("content_hash:") {
                content_hash = Some(h.trim().to_string());
            }
        }

        // Content is everything after "---\n"
        if let Some(sep) = block.find("\n---\n") {
            content = Some(block[sep + 5..].to_string());
        } else if let Some(sep) = block.find("---\n") {
            content = Some(block[sep + 4..].to_string());
        }

        if !path.is_empty() {
            actions.push(FileAction {
                path,
                operation: op,
                content,
                content_hash,
            });
        }
    }
    actions
}

/// Executes a `Vec<FileAction>` transactionally, rolling back already-applied
/// operations if any subsequent action fails.
pub fn execute_file_actions(actions: Vec<FileAction>) -> Vec<SwdTransaction> {
    let ts = now_ms();
    let mut transactions: Vec<SwdTransaction> = Vec::new();
    let mut snapshots_before: Vec<(String, Option<String>, Option<Vec<u8>>)> = Vec::new();

    // Phase 1: snapshot all before
    for action in &actions {
        let (hash, content) = snapshot(&action.path);
        snapshots_before.push((action.path.clone(), hash, content));
    }

    // Phase 2: execute
    let mut failed_at: Option<usize> = None;
    for (i, action) in actions.iter().enumerate() {
        let exec_result = execute_file_action_inner(action);
        if exec_result.is_err() {
            failed_at = Some(i);
            break;
        }
    }

    if let Some(fail_idx) = failed_at {
        // Rollback all executed actions in reverse.
        for i in (0..fail_idx).rev() {
            let (path, _, before_bytes) = &snapshots_before[i];
            let _ = rollback(path, before_bytes.as_deref());
        }
        for (i, action) in actions.iter().enumerate() {
            let outcome = if i < fail_idx {
                SwdOutcome::RolledBack
            } else if i == fail_idx {
                SwdOutcome::Failed {
                    reason: format!("execution failed for {}", action.path),
                }
            } else {
                SwdOutcome::RolledBack
            };
            let (_, before_hash, _) = &snapshots_before[i];
            transactions.push(SwdTransaction {
                tool_name: "swd_full".to_string(),
                path: action.path.clone(),
                before_hash: before_hash.clone(),
                after_hash: None,
                outcome,
                timestamp_ms: ts,
            });
        }
    } else {
        // Phase 3: verify all after.
        for (i, action) in actions.iter().enumerate() {
            let (after_hash, _) = snapshot(&action.path);
            let (_, before_hash, _) = &snapshots_before[i];

            // Validate CONTENT_HASH if declared.
            let outcome = if let Some(declared) = &action.content_hash {
                if after_hash.as_deref() != Some(declared.as_str()) {
                    SwdOutcome::Drift {
                        detail: format!(
                            "hash mismatch: declared={}, actual={}",
                            &declared[..8.min(declared.len())],
                            after_hash
                                .as_deref()
                                .map(|h| &h[..8.min(h.len())])
                                .unwrap_or("none")
                        ),
                    }
                } else {
                    verify_outcome(before_hash, &after_hash, true)
                }
            } else {
                verify_outcome(before_hash, &after_hash, true)
            };

            transactions.push(SwdTransaction {
                tool_name: "swd_full".to_string(),
                path: action.path.clone(),
                before_hash: before_hash.clone(),
                after_hash,
                outcome,
                timestamp_ms: ts,
            });
        }
    }

    transactions
}

fn execute_file_action_inner(action: &FileAction) -> io::Result<()> {
    match action.operation {
        FileOp::Write => {
            if let Some(ref content) = action.content {
                if let Some(parent) = Path::new(&action.path).parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                std::fs::write(&action.path, content)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Write action has no content",
                ))
            }
        }
        FileOp::Delete => {
            if Path::new(&action.path).exists() {
                std::fs::remove_file(&action.path)?;
            }
            Ok(())
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

// ─── Log ─────────────────────────────────────────────────────

pub fn swd_log_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".elai")
        .join("swd.log")
}

pub fn append_swd_log(txs: &[SwdTransaction]) -> io::Result<()> {
    use std::io::Write as _;
    if txs.is_empty() {
        return Ok(());
    }
    let path = swd_log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    for tx in txs {
        let line = format!(
            "{{\"ts\":{},\"tool\":\"{}\",\"path\":\"{}\",\"outcome\":\"{}\",\"before\":{},\"after\":{}}}\n",
            tx.timestamp_ms,
            tx.tool_name,
            tx.path,
            tx.outcome.as_str(),
            tx.before_hash.as_deref().map(|h| format!("\"{h}\"")).unwrap_or_else(|| "null".to_string()),
            tx.after_hash.as_deref().map(|h| format!("\"{h}\"")).unwrap_or_else(|| "null".to_string()),
        );
        file.write_all(line.as_bytes())?;
    }
    Ok(())
}

// ─── Full mode system prompt ──────────────────────────────────

pub const SWD_FULL_SYSTEM_PROMPT: &str = "\
## SWD Full Mode Active

You MUST NOT call write_file, edit_file, or any file write tool directly.
Instead, emit all filesystem changes as structured [FILE_ACTION] blocks in your text:

[FILE_ACTION:Write]
path: relative/path/to/file.rs
content_hash: <sha256-hex-of-exact-content>
---
<exact file content here>
[/FILE_ACTION]

[FILE_ACTION:Delete]
path: relative/path/to/file.rs
---
[/FILE_ACTION]

Rules:
- Use relative paths from the current working directory
- content_hash must be the SHA-256 hex of the exact bytes in the CONTENT section
- The engine executes all FILE_ACTION blocks transactionally after your response
- If any action fails, ALL previous actions in the response are rolled back
- You may still use read_file, grep_search, glob_search, bash (read-only) normally
";

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tx(outcome: SwdOutcome) -> SwdTransaction {
        SwdTransaction {
            tool_name: "write_file".to_string(),
            path: "src/foo.rs".to_string(),
            before_hash: Some("aabbcc".to_string()),
            after_hash: Some("ddeeff".to_string()),
            outcome,
            timestamp_ms: 0,
        }
    }

    #[test]
    fn test_correction_context_new() {
        let ctx = CorrectionContext::new();
        assert_eq!(ctx.attempts, 0);
        assert_eq!(ctx.max_attempts, MAX_CORRECTION_ATTEMPTS);
        assert!(ctx.can_retry());
        assert!(!ctx.has_failures());
    }

    #[test]
    fn test_correction_context_max_attempts() {
        let mut ctx = CorrectionContext::new();
        let txs = vec![make_tx(SwdOutcome::Failed {
            reason: "error".to_string(),
        })];
        ctx.record_failures(&txs);
        assert!(ctx.can_retry());
        ctx.record_failures(&txs);
        assert!(!ctx.can_retry());
    }

    #[test]
    fn test_correction_context_reset() {
        let mut ctx = CorrectionContext::new();
        let txs = vec![make_tx(SwdOutcome::Failed {
            reason: "error".to_string(),
        })];
        ctx.record_failures(&txs);
        ctx.record_failures(&txs);
        assert!(!ctx.can_retry());
        ctx.reset();
        assert_eq!(ctx.attempts, 0);
        assert!(ctx.can_retry());
        assert!(!ctx.has_failures());
    }

    #[test]
    fn test_correction_context_filters_only_failures() {
        let mut ctx = CorrectionContext::new();
        let txs = vec![
            make_tx(SwdOutcome::Verified),
            make_tx(SwdOutcome::Noop),
            make_tx(SwdOutcome::Failed {
                reason: "fail".to_string(),
            }),
            make_tx(SwdOutcome::Drift {
                detail: "drift".to_string(),
            }),
            make_tx(SwdOutcome::RolledBack),
        ];
        ctx.record_failures(&txs);
        assert_eq!(ctx.last_failures.len(), 3);
    }

    #[test]
    fn test_correction_prompt_format() {
        let txs = vec![make_tx(SwdOutcome::Failed {
            reason: "hash mismatch".to_string(),
        })];
        let prompt = build_correction_prompt(&txs);
        assert!(prompt.contains("[SWD CORRECTION TURN]"));
        assert!(prompt.contains("src/foo.rs"));
        assert!(prompt.contains("FAILED"));
        assert!(prompt.contains("hash mismatch"));
        assert!(prompt.contains("aabbcc"));
    }
}
