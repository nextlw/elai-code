//! # Script Runner
//!
//! Execute scripts (Python, Node, Rust, Shell) and optionally use LLM to
//! analyze output and apply suggested file updates.
//!
//! ## Usage
//!
//! ### CLI standalone
//! ```bash
//! cargo run --bin script-runner -- scripts/analise.py --param valor
//! cargo run --bin script-runner -- scripts/analise.py --update
//! ```
//!
//! ### As library
//! ```rust,no_run
//! use script_runner::{ScriptConfig, execute};
//!
//! let config = ScriptConfig::new("scripts/analise.py");
//! let result = execute(&config)?;
//! println!("{}", result.stdout());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod runners;
mod executor;
mod llm_updater;
mod file_actions;

pub use runners::{RunnerRegistry, ScriptRunner, RunnerKind};
pub use executor::{execute, execute_with_update, ExecutionResult, ScriptConfig};
pub use llm_updater::{
    execute_actions, FileActionParsed, LlmUpdater, LlmUpdaterConfig, LlmUpdaterResult,
};
pub use file_actions::{
    execute_action, log_actions, FileAction, FileActionResult,
};

/// Synchronous script execution for slash command integration.
/// Runs the script and returns stdout as a string, ignoring stderr unless
/// the script exits with non-zero.
///
/// `progress` is called with status messages; pass `|_| ()` when progress
/// reporting is not needed.
pub fn run_script<F>(script: &str, args: Option<&str>, config: &ScriptConfig, progress: F) -> Result<String, Box<dyn std::error::Error + Send + Sync>>
where
    F: Fn(&str),
{
    use std::process::Command;

    let script_path = std::path::Path::new(script);
    if !script_path.exists() {
        return Err(format!("script not found: {script}").into());
    }

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", &format!("{} {}", script, args.unwrap_or(""))]);
        c
    } else {
        let ext = script_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let (interpreter, script_arg) = match ext {
            "py" => ("python3", script),
            "js" => ("node", script),
            "rb" => ("ruby", script),
            "sh" => ("bash", script),
            "php" => ("php", script),
            _ => (script, ""),
        };

        let mut c = Command::new(interpreter);
        if !script_arg.is_empty() {
            c.arg(script_arg);
        }
        if let Some(a) = args {
            for arg in a.split_whitespace() {
                c.arg(arg);
            }
        }
        c
    };

    progress(&format!("Running: {script}"));
    let output = cmd
        .output()
        .map_err(|e| format!("failed to execute {script}: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        eprintln!("Script exited with code {code}");
        if !stderr.is_empty() {
            eprintln!("{stderr}");
        }
        return Err(format!("script exited with code {code}").into());
    }

    if config.update {
        progress("Update mode: would analyze output with LLM (requires API key)");
    }

    Ok(stdout.to_string())
}
