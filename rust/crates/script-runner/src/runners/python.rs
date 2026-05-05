//! Python script runner

use std::path::Path;
use std::process::Command;
use anyhow::{Context, Result};
use std::time::Instant;

use super::{Runner, RunnerKind, ExecutionOutput};

/// Python script runner
pub struct PythonRunner {
    python_cmd: String,
}

impl PythonRunner {
    pub fn new() -> Self {
        // Try python3 first, fallback to python
        let python_cmd = if Command::new("python3").arg("--version").output().is_ok() {
            "python3".to_string()
        } else {
            "python".to_string()
        };
        
        Self { python_cmd }
    }
}

impl Default for PythonRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner for PythonRunner {
    fn run(&self, script_path: &Path, args: &[&str], cwd: &Path) -> Result<ExecutionOutput> {
        let start = Instant::now();
        
        let mut cmd = Command::new(&self.python_cmd);
        cmd.arg(script_path);
        cmd.args(args);
        cmd.current_dir(cwd);
        
        // Inherit environment but could add custom vars here
        // cmd.env_remove("PYTHONDONTWRITEBYTECODE"); // Optional
        
        let output = cmd.output()
            .with_context(|| format!("Failed to execute Python script: {}", script_path.display()))?;
        
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        
        Ok(ExecutionOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            duration_ms,
        })
    }
    
    fn kind(&self) -> RunnerKind {
        RunnerKind::Python
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    
    #[test]
    fn test_python_hello_world() {
        let dir = tempdir().unwrap();
        let script = dir.path().join("test.py");
        fs::write(&script, "print('Hello from Python!')").unwrap();
        
        let runner = PythonRunner::new();
        let output = runner.run(&script, &[], dir.path()).unwrap();
        
        assert!(output.is_success());
        assert!(output.stdout.contains("Hello from Python!"));
    }
    
    #[test]
    fn test_python_with_args() {
        let dir = tempdir().unwrap();
        let script = dir.path().join("test.py");
        fs::write(&script, "import sys; print(f'Args: {sys.argv}')").unwrap();
        
        let runner = PythonRunner::new();
        let output = runner.run(&script, &["arg1", "arg2"], dir.path()).unwrap();
        
        assert!(output.is_success());
        assert!(output.stdout.contains("arg1"));
        assert!(output.stdout.contains("arg2"));
    }
    
    #[test]
    fn test_python_error() {
        let dir = tempdir().unwrap();
        let script = dir.path().join("test.py");
        fs::write(&script, "raise ValueError('test error')").unwrap();
        
        let runner = PythonRunner::new();
        let output = runner.run(&script, &[], dir.path()).unwrap();
        
        assert!(!output.is_success());
        assert!(output.stderr.contains("ValueError"));
    }
}
