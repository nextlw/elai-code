//! Node.js script runner

use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result};
use std::time::Instant;

use super::{Runner, RunnerKind, ExecutionOutput};

/// Node.js script runner
pub struct NodeRunner {
    node_cmd: String,
}

impl NodeRunner {
    pub fn new() -> Self {
        let node_cmd = if Command::new("node").arg("--version").output().is_ok() {
            "node".to_string()
        } else {
            "nodejs".to_string()
        };
        Self { node_cmd }
    }
}

impl Default for NodeRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for NodeRunner {
    fn run(&self, script_path: &Path, args: &[&str], cwd: &Path) -> Result<ExecutionOutput> {
        let start = Instant::now();
        
        let mut cmd = Command::new(&self.node_cmd);
        cmd.arg(script_path);
        cmd.args(args);
        cmd.current_dir(cwd);
        
        let output = cmd.output()
            .with_context(|| format!("Failed to execute Node script: {}", script_path.display()))?;
        
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        
        Ok(ExecutionOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            duration_ms,
        })
    }
    
    fn kind(&self) -> RunnerKind {
        RunnerKind::Node
    }
}
