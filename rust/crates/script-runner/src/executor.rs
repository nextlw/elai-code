//! Script execution pipeline
//!
//! Orchestrates script execution, result handling, and optional LLM updates.

use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use tracing::{info, warn};

use super::runners::{ScriptRunner, RunnerKind, ExecutionOutput};

/// Configuration for script execution
#[derive(Debug, Clone)]
pub struct ScriptConfig {
    /// Path to the script (relative or absolute)
    pub script_path: PathBuf,
    
    /// Arguments to pass to the script
    pub args: Vec<String>,
    
    /// Working directory (defaults to script's directory)
    pub cwd: Option<PathBuf>,
    
    /// Whether to run LLM update after execution
    pub update: bool,
    
    /// Force a specific runner kind (auto-detect if None)
    pub force_runner: Option<RunnerKind>,
    
    /// Environment variables to set
    pub env: Vec<(String, String)>,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            script_path: PathBuf::new(),
            args: Vec::new(),
            cwd: None,
            update: false,
            force_runner: None,
            env: Vec::new(),
        }
    }
}

impl ScriptConfig {
    /// Create a new config with a script path
    pub fn new(script_path: impl Into<PathBuf>) -> Self {
        Self {
            script_path: script_path.into(),
            ..Default::default()
        }
    }

    /// Add arguments to the script
    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.args.extend(args);
        self
    }

    /// Set working directory
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Enable LLM update mode
    #[must_use]
    pub fn with_update(mut self) -> Self {
        self.update = true;
        self
    }

    /// Force a specific runner
    #[must_use]
    pub fn runner(mut self, kind: RunnerKind) -> Self {
        self.force_runner = Some(kind);
        self
    }

    /// Add an environment variable
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Resolve working directory
    #[must_use]
    pub fn resolve_cwd(&self) -> PathBuf {
        self.cwd.clone()
            .or_else(|| self.script_path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    /// Resolve script path (make absolute if relative)
    pub fn resolve_script_path(&self) -> Result<PathBuf> {
        let cwd = self.resolve_cwd();
        
        if self.script_path.is_absolute() {
            Ok(self.script_path.clone())
        } else {
            Ok(cwd.join(&self.script_path))
        }
    }
}

/// Result of script execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// The execution output
    pub output: ExecutionOutput,
    
    /// Resolved script path
    pub script_path: PathBuf,
    
    /// Resolved working directory
    pub cwd: PathBuf,
    
    /// Runner kind used
    pub runner: RunnerKind,
    
    /// Execution timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ExecutionResult {
    /// Create a new execution result
    #[must_use]
    pub fn new(
        output: ExecutionOutput,
        script_path: PathBuf,
        cwd: PathBuf,
        runner: RunnerKind,
    ) -> Self {
        Self {
            output,
            script_path,
            cwd,
            runner,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Get the stdout
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.output.stdout
    }

    /// Get the stderr
    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.output.stderr
    }

    /// Check if execution was successful
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.output.is_success()
    }

    /// Get exit code
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        self.output.exit_code
    }

    /// Get execution duration in milliseconds
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.output.duration_ms
    }

    /// Format as a summary string
    #[must_use]
    pub fn summary(&self) -> String {
        let status = if self.is_success() {
            "SUCCESS"
        } else {
            "FAILED"
        };
        
        format!(
            "[{}] {} (exit={}, duration={}ms)",
            status,
            self.script_path.display(),
            self.exit_code(),
            self.duration_ms()
        )
    }
}

/// Execute a script with the given configuration
pub fn execute(config: &ScriptConfig) -> Result<ExecutionResult> {
    let cwd = config.resolve_cwd();
    let script_path = config.resolve_script_path()
        .with_context(|| format!("Failed to resolve script path: {}", config.script_path.display()))?;
    
    info!("Executing script: {} (cwd: {})", script_path.display(), cwd.display());
    
    // Validate script exists
    if !script_path.exists() {
        anyhow::bail!("Script not found: {}", script_path.display());
    }
    
    let runner = ScriptRunner::new();
    
    // Determine runner
    let runner_kind = config.force_runner
        .unwrap_or_else(|| {
            // Try to detect from extension
            let ext = script_path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            
            match ext.to_lowercase().as_str() {
                "py" => RunnerKind::Python,
                "js" | "mjs" | "ts" => RunnerKind::Node,
                "rs" => RunnerKind::RustScript,
                _ => RunnerKind::Shell,
            }
        });
    
    info!("Using runner: {runner_kind:?}");

    let args: Vec<&str> = config.args.iter().map(String::as_str).collect();
    
    // Execute
    let output = if let Some(kind) = config.force_runner {
        runner.execute_with(kind, &script_path, &args, &cwd)
    } else {
        runner.execute(&script_path, &args, &cwd)
    };
    
    match output {
        Ok(output) => {
            if output.is_success() {
                info!("Script executed successfully");
            } else {
                warn!(
                    "Script failed with exit code {}: {}",
                    output.exit_code,
                    output.stderr
                );
            }
            
            Ok(ExecutionResult::new(output, script_path, cwd, runner_kind))
        }
        Err(e) => {
            warn!("Script execution failed: {}", e);
            Err(e)
        }
    }
}

/// Execute and optionally update
pub fn execute_with_update(
    config: &ScriptConfig,
) -> Result<(ExecutionResult, Option<crate::llm_updater::LlmUpdaterResult>)> {
    // First, execute the script
    let result = execute(config)?;
    
    if !config.update {
        return Ok((result, None));
    }
    
    // If update is enabled and script succeeded, call LLM
    if !result.is_success() {
        warn!("Skipping LLM update because script failed");
        return Ok((result, None));
    }
    
    let updater = crate::LlmUpdater::new();
    let update_result = updater.analyze_and_suggest(&result).ok();

    Ok((result, update_result))
}
