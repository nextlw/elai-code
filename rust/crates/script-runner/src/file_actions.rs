//! File actions — parse and execute file updates from LLM suggestions.
//!
//! Provides utilities for parsing structured file action blocks
//! (similar to SWD's `[FILE_ACTION]` blocks) and executing them safely.

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use sha2::{Sha256, Digest};
use tracing::{info, warn};

/// Represents a parsed file action
#[derive(Debug, Clone)]
pub struct FileAction {
    /// Action type
    pub action_type: FileActionType,

    /// Target file path (relative or absolute)
    pub path: PathBuf,

    /// Reason/explanation for this action
    pub reason: String,

    /// Content for write/create actions
    pub content: Option<String>,

    /// SHA-256 hash of the content (for verification)
    pub content_hash: Option<String>,

    /// Line range for edit actions (start, end)
    pub line_range: Option<(usize, usize)>,
}

impl FileAction {
    /// Parse from a [`crate::llm_updater::FileActionParsed`]
    pub fn from_parsed(parsed: &crate::llm_updater::FileActionParsed) -> Result<Self> {
        let action_type = match parsed.action_type.to_lowercase().as_str() {
            "write_file" => FileActionType::Write,
            "edit_file" => FileActionType::Edit,
            "create_dir" | "mkdir" => FileActionType::CreateDir,
            "delete_file" | "delete" => FileActionType::Delete,
            other => anyhow::bail!("Unknown action type: {other}"),
        };

        Ok(Self {
            action_type,
            path: PathBuf::from(&parsed.file_path),
            reason: parsed.reason.clone(),
            content: parsed.content.clone(),
            content_hash: parsed
                .content
                .as_ref()
                .map(|c| compute_hash(c.as_str())),
            line_range: parsed.line_range,
        })
    }
}

/// Type of file action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileActionType {
    /// Write/create a file entirely
    Write,
    /// Edit specific lines in a file
    Edit,
    /// Create a directory
    CreateDir,
    /// Delete a file
    Delete,
}

/// Result of executing a file action
#[derive(Debug, Clone)]
pub struct FileActionResult {
    /// The action that was executed
    pub action: FileAction,

    /// Whether execution was successful
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Hash of content before (if applicable)
    pub before_hash: Option<String>,

    /// Hash of content after (if applicable)
    pub after_hash: Option<String>,
}

/// Execute a single file action
pub fn execute_action(action: &FileAction, cwd: &Path) -> Result<FileActionResult> {
    let full_path = cwd.join(&action.path);

    // Compute before hash if file exists
    let before_hash = if full_path.exists() {
        compute_file_hash(&full_path).ok()
    } else {
        None
    };

    let result = match action.action_type {
        FileActionType::Write => execute_write(action, &full_path),
        FileActionType::Edit => execute_edit(action, &full_path),
        FileActionType::CreateDir => execute_mkdir(action, &full_path),
        FileActionType::Delete => execute_delete(action, &full_path),
    };

    let after_hash = result.is_ok().then(|| compute_file_hash(&full_path).ok()).flatten();

    Ok(FileActionResult {
        action: action.clone(),
        success: result.is_ok(),
        error: result.err().map(|e| e.to_string()),
        before_hash,
        after_hash,
    })
}

/// Execute a write action
fn execute_write(action: &FileAction, path: &Path) -> Result<()> {
    let content = action.content.as_deref().unwrap_or("");

    // Verify content hash if provided
    if let Some(expected_hash) = &action.content_hash {
        let actual_hash = compute_hash(content);
        if &actual_hash != expected_hash {
            anyhow::bail!(
                "Content hash mismatch for {}: expected {}, got {}",
                path.display(),
                expected_hash,
                actual_hash
            );
        }
    }

    // Create parent directories
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Write the file
    std::fs::write(path, content)
        .with_context(|| format!("Failed to write file: {}", path.display()))?;

    info!("Wrote file: {} ({} bytes)", path.display(), content.len());

    Ok(())
}

/// Execute an edit action
fn execute_edit(action: &FileAction, path: &Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("Cannot edit non-existent file: {}", path.display());
    }

    let new_content = action.content.as_deref().unwrap_or("");

    match action.line_range {
        Some((start, end)) if start > 0 => {
            let original = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {}", path.display()))?;

            let mut lines: Vec<&str> = original.lines().collect();
            // Convert 1-based inclusive range to 0-based indices
            let from = (start - 1).min(lines.len());
            let to = end.min(lines.len());

            let replacement: Vec<&str> = new_content.lines().collect();
            lines.splice(from..to, replacement);

            let result = lines.join("\n");
            // Preserve trailing newline if original had one
            let result = if original.ends_with('\n') {
                format!("{result}\n")
            } else {
                result
            };

            std::fs::write(path, result)
                .with_context(|| format!("Failed to write file: {}", path.display()))?;

            info!("Edited lines {start}–{end} in: {}", path.display());
        }
        _ => {
            // No line_range: full overwrite
            std::fs::write(path, new_content)
                .with_context(|| format!("Failed to write file: {}", path.display()))?;

            info!("Overwrote file: {}", path.display());
        }
    }

    Ok(())
}

/// Execute a mkdir action
fn execute_mkdir(_action: &FileAction, path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory: {}", path.display()))?;

    info!("Created directory: {}", path.display());

    Ok(())
}

/// Execute a delete action
fn execute_delete(_action: &FileAction, path: &Path) -> Result<()> {
    if !path.exists() {
        warn!("Cannot delete non-existent file: {}", path.display());
        return Ok(());
    }

    std::fs::remove_file(path)
        .with_context(|| format!("Failed to delete file: {}", path.display()))?;

    info!("Deleted file: {}", path.display());

    Ok(())
}

/// Compute SHA-256 hash of a string
pub fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Compute SHA-256 hash of a file
pub fn compute_file_hash(path: &Path) -> Result<String> {
    let content = std::fs::read(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Log actions to a file (similar to SWD's swd.log)
pub fn log_actions(actions: &[FileActionResult], log_path: &Path) -> Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;

    let timestamp = chrono::Utc::now().to_rfc3339();

    for result in actions {
        let outcome = if result.success { "Success" } else { "Failed" };
        let before = result.before_hash.as_deref().unwrap_or("-");
        let after = result.after_hash.as_deref().unwrap_or("-");

        let log_entry = format!(
            r#"{{"ts":{},"tool":"{}","path":"{}","outcome":"{}","before":"{}","after":"{}","reason":"{}"}}
"#,
            timestamp,
            result.action.action_type.debug_name(),
            result.action.path.display(),
            outcome,
            before,
            after,
            result.action.reason.replace('"', "'")
        );

        file.write_all(log_entry.as_bytes())
            .context("Failed to write to log")?;
    }

    Ok(())
}

impl FileActionType {
    /// Get a debug-friendly name for logging
    #[must_use]
    pub fn debug_name(self) -> &'static str {
        match self {
            Self::Write => "write_file",
            Self::Edit => "edit_file",
            Self::CreateDir => "create_dir",
            Self::Delete => "delete_file",
        }
    }
}
