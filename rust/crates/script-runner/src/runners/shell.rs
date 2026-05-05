//! Shell script runner

use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result};
use std::time::Instant;

use super::{Runner, RunnerKind, ExecutionOutput};

/// Shell script runner
pub struct ShellRunner {
    shell_cmd: String,
}

impl ShellRunner {
    pub fn new() -> Self {
        // Try bash first, fallback to sh
        let shell_cmd = if Command::new("bash").arg("--version").output().is_ok() {
            "bash".to_string()
        } else {
            "sh".to_string()
        };
        Self { shell_cmd }
    }
}

impl Default for ShellRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for ShellRunner {
    fn run(&self, script_path: &Path, args: &[&str], cwd: &Path) -> Result<ExecutionOutput> {
        let start = Instant::now();
        
        let mut cmd = Command::new(&self.shell_cmd);
        
        // For .sh scripts, source them; for others, execute
        let ext = script_path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        
        if ext == "sh" || ext == "bash" {
            cmd.arg(script_path);
        } else {
            cmd.arg("-c");
            // Read script and execute inline
            let script_content = std::fs::read_to_string(script_path)
                .with_context(|| format!("Failed to read script: {}", script_path.display()))?;
            cmd.arg(script_content);
        }
        
        cmd.args(args);
        cmd.current_dir(cwd);
        
        let output = cmd.output()
            .with_context(|| format!("Failed to execute shell script: {}", script_path.display()))?;
        
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        
        Ok(ExecutionOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            duration_ms,
        })
    }
    
    fn kind(&self) -> RunnerKind {
        RunnerKind::Shell
    }
}
