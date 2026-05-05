//! Rust script runner (via rust-script or cargo-script)

use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result};
use std::time::Instant;

use super::{Runner, RunnerKind, ExecutionOutput};

/// Rust script runner using rust-script
pub struct RustScriptRunner {
    use_cargo_script: bool,
}

impl RustScriptRunner {
    pub fn new() -> Self {
        // Prefer cargo-script if available, fallback to rust-script
        let use_cargo_script = Command::new("cargo-script").arg("--version").output().is_ok();
        Self { use_cargo_script }
    }
}

impl Default for RustScriptRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for RustScriptRunner {
    fn run(&self, script_path: &Path, args: &[&str], cwd: &Path) -> Result<ExecutionOutput> {
        let start = Instant::now();
        
        let mut cmd = if self.use_cargo_script {
            let mut c = Command::new("cargo-script");
            c.arg("run");
            c
        } else {
            Command::new("rust-script")
        };
        
        cmd.arg(script_path);
        cmd.args(args);
        cmd.current_dir(cwd);
        
        let output = cmd.output()
            .with_context(|| format!("Failed to execute Rust script: {}", script_path.display()))?;
        
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        
        Ok(ExecutionOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            duration_ms,
        })
    }
    
    fn kind(&self) -> RunnerKind {
        RunnerKind::RustScript
    }
}
