//! Script Runner CLI - Standalone binary for running scripts with LLM updates
//!
//! This binary can be run via:
//! ```bash
//! cargo run --bin script-runner -- scripts/analise.py --param valor
//! cargo run --bin script-runner -- scripts/analise.py --update
//! ```

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use tracing::{info, error, Level};
use tracing_subscriber::FmtSubscriber;

use script_runner::{
    execute, execute_action, log_actions, FileAction, LlmUpdater, RunnerKind,
    ScriptConfig,
};

#[derive(Parser, Debug)]
#[command(name = "script-runner")]
#[command(about = "Execute scripts and use LLM to apply suggested updates", long_about = None)]
#[allow(clippy::struct_field_names)]
struct Args {
    /// Path to the script to execute
    script: PathBuf,

    /// Arguments to pass to the script
    #[arg(trailing_var_arg = true)]
    args: Vec<String>,

    /// Enable LLM update mode
    #[arg(short, long, default_value = "false")]
    update: bool,

    /// Force a specific runner
    #[arg(short, long, value_enum, default_value = "auto")]
    runner: Option<RunnerArg>,

    /// Working directory (defaults to script's directory)
    #[arg(short, long)]
    cwd: Option<PathBuf>,

    /// Log file for actions (default: .elai/script-actions.log)
    #[arg(long)]
    log_file: Option<PathBuf>,

    /// Enable verbose output
    #[arg(short, long, default_value = "false")]
    verbose: bool,

    /// Dry run - don't execute file actions
    #[arg(long, default_value = "false")]
    dry_run: bool,
}

#[derive(Debug, Clone, ValueEnum)]
enum RunnerArg {
    Python,
    Node,
    RustScript,
    Shell,
    Auto,
}

impl RunnerArg {
    fn to_kind(&self) -> Option<RunnerKind> {
        match self {
            RunnerArg::Python => Some(RunnerKind::Python),
            RunnerArg::Node => Some(RunnerKind::Node),
            RunnerArg::RustScript => Some(RunnerKind::RustScript),
            RunnerArg::Shell => Some(RunnerKind::Shell),
            RunnerArg::Auto => None,
        }
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Setup logging
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    
    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .finish();
    
    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set tracing subscriber")?;

    info!("Script Runner starting...");
    info!("Script: {}", args.script.display());

    // Build config
    let mut config = ScriptConfig::new(&args.script);
    config.args.clone_from(&args.args);
    config.update = args.update;
    
    if let Some(cwd) = &args.cwd {
        config.cwd = Some(cwd.clone());
    }
    
    if let Some(runner) = &args.runner {
        config.force_runner = runner.to_kind();
    }

    // Execute script
    info!("Executing script...");
    let result = execute(&config)?;
    
    // Print result
    println!("\n{}", "=".repeat(60));
    println!("Script Execution Result");
    println!("{}", "=".repeat(60));
    println!("Status: {}", if result.is_success() { "✅ SUCCESS" } else { "❌ FAILED" });
    println!("Exit Code: {}", result.exit_code());
    println!("Duration: {}ms", result.duration_ms());
    
    if !result.stdout().is_empty() {
        println!("\n--- STDOUT ---");
        println!("{}", result.stdout());
    }
    
    if !result.stderr().is_empty() {
        println!("\n--- STDERR ---");
        println!("{}", result.stderr());
    }

    // If update mode and script succeeded, call LLM
    if args.update && result.is_success() {
        println!("\n{}", "=".repeat(60));
        println!("LLM Analysis");
        println!("{}", "=".repeat(60));
        
        let updater = LlmUpdater::new();
        match updater.analyze_and_suggest(&result) {
            Ok(llm_result) => {
                if llm_result.has_suggestions {
                    println!("\n{} suggestion(s) found:\n", llm_result.actions.len());
                    
                    for (i, action) in llm_result.actions.iter().enumerate() {
                        println!("{}. [{}] {}", i + 1, action.action_type, action.file_path);
                        println!("   Reason: {}", action.reason);
                        if action.content.is_some() {
                            println!("   (has content)");
                        }
                        println!();
                    }
                    
                    if args.dry_run {
                        println!("(Dry run - no actions executed)");
                    } else {
                        println!("Executing actions...");
                        let cwd = config.resolve_cwd();

                        let file_actions: Result<Vec<FileAction>, _> = llm_result.actions
                            .iter()
                            .map(FileAction::from_parsed)
                            .collect();

                        let file_actions = file_actions.context("Failed to parse actions")?;

                        let mut results = Vec::new();
                        for action in &file_actions {
                            let result = execute_action(action, &cwd)?;
                            results.push(result);
                        }

                        let log_path = args.log_file
                            .unwrap_or_else(|| cwd.join(".elai").join("script-actions.log"));

                        if let Some(parent) = log_path.parent() {
                            std::fs::create_dir_all(parent).ok();
                        }

                        log_actions(&results, &log_path)?;

                        println!("\n{}", "=".repeat(60));
                        println!("Action Results");
                        println!("{}", "=".repeat(60));

                        for result in &results {
                            let status = if result.success { "✅" } else { "❌" };
                            println!("{} {} - {}", status, result.action.path.display(),
                                result.error.as_deref().unwrap_or("OK"));
                        }

                        println!("\nLogged to: {}", log_path.display());
                    }
                } else {
                    println!("No file updates needed.");
                }
                
                if !llm_result.raw_response.is_empty() && args.verbose {
                    println!("\n--- RAW LLM RESPONSE ---");
                    println!("{}", llm_result.raw_response);
                }
            }
            Err(e) => {
                error!("LLM analysis failed: {e}");
                eprintln!("\n❌ LLM analysis failed: {e}\n");
            }
        }
    }

    println!("\n{}", "=".repeat(60));
    println!("Done!");
    println!("{}", "=".repeat(60));

    // Exit with script's exit code
    if result.is_success() {
        Ok(())
    } else {
        std::process::exit(result.exit_code());
    }
}
