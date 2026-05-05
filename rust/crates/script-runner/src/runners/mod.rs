//! Script runners module
//!
//! Provides runners for different script languages:
//! - Python (.py)
//! - Node/JS (.js, .mjs)
//! - Rust scripts (.rs via cargo-script or rust-script)
//! - Shell (.sh, .bash)

pub mod python;
pub mod node;
pub mod rust_script;
pub mod shell;

use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result};

/// Supported runner kinds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerKind {
    Python,
    Node,
    RustScript,
    Shell,
}

/// Detect runner kind from file extension
pub fn detect_runner_kind(script_path: &Path) -> Option<RunnerKind> {
    let ext = script_path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "py" => Some(RunnerKind::Python),
        "js" | "mjs" | "ts" => Some(RunnerKind::Node),
        "rs" => Some(RunnerKind::RustScript),
        "sh" | "bash" | "zsh" => Some(RunnerKind::Shell),
        _ => None,
    }
}

/// Check if a runner is available on the system
pub fn is_runner_available(kind: RunnerKind) -> bool {
    match kind {
        RunnerKind::Python => Command::new("python3").arg("--version").output().is_ok(),
        RunnerKind::Node => Command::new("node").arg("--version").output().is_ok(),
        RunnerKind::RustScript => {
            Command::new("cargo").arg("--version").output().is_ok()
                && Command::new("rust-script").arg("--version").output().is_ok()
        }
        RunnerKind::Shell => true, // Always available as /bin/sh
    }
}

/// Trait for script runners
pub trait Runner {
    /// Execute the script with given arguments
    fn run(&self, script_path: &Path, args: &[&str], cwd: &Path) -> Result<ExecutionOutput>;
    
    /// Get the runner kind
    fn kind(&self) -> RunnerKind;
}

/// Execution output from a script
#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

impl ExecutionOutput {
    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }

    /// Get combined output (stdout + stderr)
    pub fn combined_output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else {
            format!("{}\n\nSTDERR:\n{}", self.stdout, self.stderr)
        }
    }
}

/// Runner registry - manages all available runners
pub struct RunnerRegistry {
    runners: Vec<Box<dyn Runner>>,
}

impl Default for RunnerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RunnerRegistry {
    /// Create a new registry with all available runners
    #[must_use]
    pub fn new() -> Self {
        let mut runners: Vec<Box<dyn Runner>> = Vec::new();
        
        if is_runner_available(RunnerKind::Python) {
            runners.push(Box::new(python::PythonRunner::new()));
        }
        if is_runner_available(RunnerKind::Node) {
            runners.push(Box::new(node::NodeRunner::new()));
        }
        if is_runner_available(RunnerKind::RustScript) {
            runners.push(Box::new(rust_script::RustScriptRunner::new()));
        }
        runners.push(Box::new(shell::ShellRunner::new()));
        
        Self { runners }
    }

    /// Get runner for a specific kind
    #[must_use]
    pub fn get(&self, kind: RunnerKind) -> Option<&dyn Runner> {
        self.runners.iter().find(|r| r.kind() == kind).map(AsRef::as_ref)
    }

    /// Auto-detect runner from script path
    pub fn detect(&self, script_path: &Path) -> Result<&dyn Runner> {
        let kind = detect_runner_kind(script_path)
            .context("Unsupported script extension")?;

        self.get(kind)
            .with_context(|| format!("Runner for {kind:?} not available"))
    }

    /// List all available runners
    #[must_use]
    pub fn available_runners(&self) -> Vec<RunnerKind> {
        self.runners.iter().map(|r| r.kind()).collect()
    }
}

/// Simple script runner that auto-detects language
pub struct ScriptRunner {
    registry: RunnerRegistry,
}

impl Default for ScriptRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptRunner {
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: RunnerRegistry::new(),
        }
    }

    /// Execute a script, auto-detecting the runner
    pub fn execute(&self, script_path: &Path, args: &[&str], cwd: &Path) -> Result<ExecutionOutput> {
        self.registry.detect(script_path)?.run(script_path, args, cwd)
    }

    /// Execute with explicit runner kind
    pub fn execute_with(&self, kind: RunnerKind, script_path: &Path, args: &[&str], cwd: &Path) -> Result<ExecutionOutput> {
        self.registry.get(kind)
            .context("Runner not available")?
            .run(script_path, args, cwd)
    }

    /// List available runners
    #[must_use]
    pub fn available_runners(&self) -> Vec<RunnerKind> {
        self.registry.available_runners()
    }
}
