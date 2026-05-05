//! LLM Updater - Uses AI to analyze script output and suggest file updates
//!
//! This module integrates with the existing API crate to call an LLM,
//! analyze script execution output, and generate file update suggestions.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::executor::ExecutionResult;

/// Result from LLM analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmUpdaterResult {
    /// The raw response from the LLM
    pub raw_response: String,
    
    /// Parsed file actions suggested by the LLM
    pub actions: Vec<FileActionParsed>,
    
    /// Whether the LLM found any issues or suggestions
    pub has_suggestions: bool,
    
    /// Summary of the analysis
    pub summary: String,
}

/// A parsed file action from LLM response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileActionParsed {
    /// Action type
    pub action_type: String,
    
    /// Target file path
    pub file_path: String,
    
    /// Reason for this action
    pub reason: String,
    
    /// Suggested content (for write/edit actions)
    pub content: Option<String>,
    
    /// Line range (for edit actions)
    pub line_range: Option<(usize, usize)>,
}

/// LLM Updater configuration
#[derive(Debug, Clone)]
pub struct LlmUpdaterConfig {
    /// Model to use (defaults to config default)
    pub model: Option<String>,
    
    /// Temperature for generation (0.0 to 1.0)
    pub temperature: f32,
    
    /// Max tokens in response
    pub max_tokens: u32,
    
    /// Additional instructions for the LLM
    pub custom_instructions: Option<String>,
}

impl Default for LlmUpdaterConfig {
    fn default() -> Self {
        Self {
            model: None,
            temperature: 0.3, // Lower temp for more deterministic output
            max_tokens: 4096,
            custom_instructions: None,
        }
    }
}

/// LLM Updater - analyzes script output and suggests file updates
pub struct LlmUpdater {
    config: LlmUpdaterConfig,
}

impl LlmUpdater {
    /// Create a new LLM updater with default config
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: LlmUpdaterConfig::default(),
        }
    }

    /// Create with custom config
    #[must_use]
    pub fn with_config(config: LlmUpdaterConfig) -> Self {
        Self { config }
    }

    /// Analyze script execution result and generate update suggestions
    pub fn analyze_and_suggest(&self, result: &ExecutionResult) -> Result<LlmUpdaterResult> {
        info!("Analyzing script output with LLM...");
        
        // Build the prompt
        let prompt = self.build_prompt(result);
        
        // Call the API
        let response = Self::call_llm(&prompt);
        
        let parsed = Self::parse_response(&response);
        info!("LLM analysis complete: {} actions suggested", parsed.actions.len());
        
        Ok(parsed)
    }

    /// Build the prompt for LLM analysis
    fn build_prompt(&self, result: &ExecutionResult) -> String {
        let script_name = result.script_path.file_name()
            .map_or_else(|| "unknown".to_string(), |n| n.to_string_lossy().into_owned());
        
        let exit_status = if result.is_success() {
            "SUCCESS (exit code 0)"
        } else {
            &format!("FAILED (exit code {})", result.exit_code())
        };
        
        let custom_instructions = self.config.custom_instructions
            .as_ref()
            .map(|s| format!("\n\nAdditional instructions:\n{s}"))
            .unwrap_or_default();
        
        format!(
            r"## Script Execution Analysis

### Script: {script_name}
**Status:** {exit_status}
**Duration:** {duration_ms}ms

### Script Output:
```
{stdout}
```

### Script Errors (if any):
```
{stderr}
```

### Task
Analyze the script execution output and identify any:
1. Generated data, analysis results, or reports that should be saved to files
2. Configuration changes needed based on the output
3. Documentation updates required
4. Code improvements suggested by linters, formatters, or analysis tools
5. New files that should be created to capture the output

For each suggestion, output a structured action block:

```
[FILE_ACTION:{{action_type}}]
path: relative/path/to/file.ext
reason: Brief explanation of why this file should be updated
---
{{content_or_patch_here}}
[/FILE_ACTION]
```

Action types:
- `write_file` - Create a new file or overwrite entirely
- `edit_file` - Modify specific lines (include line numbers in comment)
- `create_dir` - Create a directory

If no file updates are needed, output:

```
[NO_ACTIONS]
The script output does not require any file updates.
[/NO_ACTIONS]
```
{custom_instructions}

Remember: Only suggest actions that are clearly indicated by the script output.
Be conservative and specific - avoid unnecessary changes.
",
            script_name = script_name,
            exit_status = exit_status,
            duration_ms = result.duration_ms(),
            stdout = result.stdout(),
            stderr = result.stderr(),
            custom_instructions = custom_instructions
        )
    }

    /// Call the LLM API.
    ///
    /// Not yet wired to the `api` crate — callers receive an empty string and
    /// `analyze_and_suggest` returns a result with `has_suggestions = false`.
    /// Wire via `OpenAiCompatClient` (see plan `llm-compact-summary`) before
    /// enabling `--update` in production.
    fn call_llm(_prompt: &str) -> String {
        warn!("LLM API call not yet implemented — returning empty result");
        String::new()
    }

    /// Parse the LLM response into structured actions
    fn parse_response(response: &str) -> LlmUpdaterResult {
        let mut actions = Vec::new();
        let mut has_suggestions = false;
        
        // Parse [FILE_ACTION] blocks
        let mut remaining = response;
        
        while let Some(start) = remaining.find("[FILE_ACTION:") {
            has_suggestions = true;
            remaining = &remaining[start + 13..]; // Skip "[FILE_ACTION:"
            
            // Get action type
            let end = remaining.find(']').unwrap_or(remaining.len());
            let action_type = remaining[..end].trim().to_string();
            remaining = &remaining[end + 1..];
            
            // Find the closing [/FILE_ACTION]
            let close_pos = remaining.find("[/FILE_ACTION]");
            
            if let Some(close_pos) = close_pos {
                let block = &remaining[..close_pos];
                remaining = &remaining[close_pos + 14..]; // Skip [/FILE_ACTION]
                
                let action = parse_action_block(&action_type, block);
                actions.push(action);
            }
        }
        
        // Check for NO_ACTIONS marker
        let no_actions = response.contains("[NO_ACTIONS]");
        let action_count = actions.len();

        LlmUpdaterResult {
            raw_response: response.to_string(),
            actions,
            has_suggestions: has_suggestions && !no_actions,
            summary: if no_actions {
                "No file updates needed based on script output.".to_string()
            } else if action_count == 0 {
                "Could not parse LLM response.".to_string()
            } else {
                format!("{action_count} file action(s) suggested")
            },
        }
    }

}

impl Default for LlmUpdater {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_action_block(action_type: &str, block: &str) -> FileActionParsed {
        let mut file_path = String::new();
        let mut reason = String::new();
        let mut content = Option::<String>::None;
        let mut line_range = Option::<(usize, usize)>::None;
        
        // Parse the block
        for line in block.lines() {
            let line = line.trim();
            
            if line.starts_with("path:") {
                file_path = line.strip_prefix("path:").unwrap_or("").trim().to_string();
            } else if line.starts_with("reason:") {
                reason = line.strip_prefix("reason:").unwrap_or("").trim().to_string();
            } else if line.starts_with("line_range:") {
                // Parse line range like "line_range: 10-20"
                let raw = line.strip_prefix("line_range:").unwrap_or("").trim();
                if let Some((s, e)) = raw.split_once('-') {
                    let start: usize = s.trim().parse().unwrap_or(0);
                    let end: usize = e.trim().parse().unwrap_or(0);
                    if start > 0 && end >= start {
                        line_range = Some((start, end));
                    }
                }
            } else if line == "---" {
                // Content follows
                let parts: Vec<&str> = block.split("---").collect();
                if parts.len() > 1 {
                    content = Some(parts[1..].join("---").trim().to_string());
                }
                break;
            }
        }
        
        FileActionParsed {
            action_type: action_type.to_string(),
            file_path,
            reason,
            content,
            line_range,
        }
}

/// Execute file actions from LLM suggestions
#[must_use]
pub fn execute_actions(actions: &[FileActionParsed], cwd: &std::path::Path) -> Vec<ActionResult> {
    actions.iter().map(|action| {
        let result = match action.action_type.as_str() {
            "write_file" => execute_write(&action.file_path, action.content.as_deref(), cwd),
            "edit_file" => execute_edit(&action.file_path, action.content.as_deref(), action.line_range, cwd),
            "create_dir" => execute_create_dir(&action.file_path, cwd),
            _ => Err(anyhow::anyhow!("Unknown action type: {}", action.action_type)),
        };
        ActionResult {
            action: action.clone(),
            success: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
        }
    }).collect()
}

/// Result of executing a single action
#[derive(Debug, Clone)]
pub struct ActionResult {
    pub action: FileActionParsed,
    pub success: bool,
    pub error: Option<String>,
}

fn execute_write(path: &str, content: Option<&str>, cwd: &std::path::Path) -> Result<()> {
    let full_path = cwd.join(path);
    
    // Create parent directories if needed
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    // Write the file
    std::fs::write(&full_path, content.unwrap_or(""))?;
    
    Ok(())
}

fn execute_edit(path: &str, content: Option<&str>, _line_range: Option<(usize, usize)>, cwd: &std::path::Path) -> Result<()> {
    let full_path = cwd.join(path);
    
    // For now, just write the content
    // A full implementation would handle line ranges properly
    std::fs::write(&full_path, content.unwrap_or(""))?;
    
    Ok(())
}

fn execute_create_dir(path: &str, cwd: &std::path::Path) -> Result<()> {
    let full_path = cwd.join(path);
    std::fs::create_dir_all(&full_path)?;
    Ok(())
}
