mod args;
mod auth;
mod diff;
mod dream;
mod init;
mod input;
mod render;
mod swd;
mod tips;
mod tui;
mod tui_sink;
mod updater;
mod verify;

// Reaponta o `t!()` deste crate para o mesmo catálogo usado por `commands`.
// `rust-i18n` exige `i18n!()` em cada crate que invoca a macro `t!()`; o
// catálogo é compartilhado (mesmo locale global).
rust_i18n::i18n!("../../locales", fallback = "en");

use std::collections::BTreeSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use api::{
    default_thinking_config, max_tokens_for_model, resolve_model_alias, resolve_output_config,
    suggested_default_model, ContentBlockDelta, EffortLevel, InputContentBlock, InputMessage,
    MessageRequest, MessageResponse, OutputContentBlock, ProviderClient,
    StreamEvent as ApiStreamEvent, ThinkingConfig, ToolChoice, ToolDefinition,
    ToolResultContentBlock,
};

use commands::{
    handle_agents_slash_command, handle_commit_push_pr_slash_command, handle_plugins_slash_command,
    handle_skills_slash_command, handle_tools_slash_command, render_slash_command_help,
    resume_supported_slash_commands, slash_command_specs, try_user_command, CommitPushPrRequest,
    SlashCommand, UserCommandRegistry,
};
use compat_harness::{extract_manifest, UpstreamPaths};
use init::initialize_repo;
use plugins::{PluginManager, PluginManagerConfig};
use render::{MarkdownStreamState, Spinner, TerminalRenderer};
use runtime::{
    check_rate_limit, generate_cache_key, load_budget_config, load_system_prompt, now_millis,
    save_budget_config, ApiClient, ApiRequest, AssistantEvent, BudgetConfig, BudgetStatus,
    BudgetTracker, BudgetUsagePct, CachedResponse, CompactionConfig, ConfigLoader, ConfigSource,
    ContentBlock, ConversationMessage, ConversationRuntime, McpServerManager,
    MessageRole, PermissionMode, PermissionPolicy, ProjectContext, ResponseCache,
    RuntimeError, Session, TelemetryEvent, TelemetryHandle, TelemetryShutdown, TelemetryWorker,
    TokenUsage, ToolError, ToolExecutor, UsageTracker,
};
use tools::{GlobalToolRegistry, MatcherPattern, McpToolSource};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use serde_json::json;

const DEFAULT_DATE: &str = "2026-03-31";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_TARGET: Option<&str> = option_env!("TARGET");
const GIT_SHA: Option<&str> = option_env!("GIT_SHA");
const INTERNAL_PROGRESS_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);

type AllowedToolSet = Vec<MatcherPattern>;

fn main() {
    if let Err(error) = run() {
        eprintln!(
            "error: {error}

Run `elai --help` for usage."
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    load_workspace_dotenv();
    init_locale();
    let args: Vec<String> = env::args().skip(1).collect();
    let action = parse_args(&args)?;
    // Skip automatic update enforcement for Repl — TUI mode shows a non-blocking
    // in-UI notification via a background thread instead (see run_tui_repl).
    if !matches!(
        action,
        CliAction::Update
            | CliAction::Uninstall
            | CliAction::Version
            | CliAction::Help
            | CliAction::Repl { .. }
    ) {
        updater::check_and_enforce();
    }
    match action {
        CliAction::DumpManifests => dump_manifests(),
        CliAction::BootstrapPlan => print_bootstrap_plan(),
        CliAction::Agents { args } => LiveCli::print_agents(args.as_deref())?,
        CliAction::Skills { args } => LiveCli::print_skills(args.as_deref())?,
        CliAction::PrintSystemPrompt { cwd, date } => print_system_prompt(cwd, date),
        CliAction::Version => print_version(),
        CliAction::ResumeSession {
            session_path,
            commands,
        } => resume_session(&session_path, &commands),
        CliAction::Prompt {
            prompt,
            model,
            output_format,
            allowed_tools,
            permission_mode,
        } => LiveCli::new(model, true, allowed_tools, permission_mode, false)?
            .run_turn_with_output(&prompt, output_format)?,
        CliAction::Login(args) => auth::dispatch_login(&args).map_err(|e| e.to_string())?,
        CliAction::Logout => auth::dispatch_logout().map_err(|e| e.to_string())?,
        CliAction::Auth { cmd } => match cmd {
            crate::args::AuthCmd::Status { json } => {
                auth::dispatch_auth_status(json).map_err(|e| e.to_string())?
            }
            crate::args::AuthCmd::List => {
                auth::dispatch_auth_list().map_err(|e| e.to_string())?
            }
        },
        CliAction::Init(args) => run_init(&args)?,
        CliAction::Repl {
            model,
            allowed_tools,
            permission_mode,
            no_tui,
            no_cache,
            swd_level,
            budget_config,
        } => run_repl(model, allowed_tools, permission_mode, no_tui, no_cache, swd_level, budget_config)?,
        CliAction::Help => print_help(),
        CliAction::Stats { days, by_model, by_project } => run_stats_command(days, by_model, by_project),
        CliAction::Verify => run_verify_command()?,
        CliAction::Update => updater::run_update(),
        CliAction::Uninstall => {
            let report = perform_uninstall();
            println!("{report}");
        }
        CliAction::Send { message, wait, json, thinking_budget } => run_headless_send(&message, wait, json, thinking_budget)?,
        CliAction::ChatShow { last, json } => run_chat_show(last, json)?,
        CliAction::ModelGet => run_model_get()?,
        CliAction::ModelSet { model } => run_model_set(&model)?,
        CliAction::Reply { answer } => run_reply(&answer)?,
        CliAction::StatusCmd { json } => run_status_cmd(json)?,
    }
    Ok(())
}

/// Loads API keys in priority order:
/// 1. `~/.elai/.env`  — global user config written by the installer
/// 2. First `.env` found walking up from the current directory (project-level override)
///
/// Keys already set in the process environment are never overwritten, so
/// an explicit `export` in the shell always wins.
fn load_workspace_dotenv() {
    // 1. Global user config (~/.elai/.env)
    if let Some(home) = dirs_home() {
        let global = home.join(".elai").join(".env");
        if global.is_file() {
            let _ = dotenvy::from_path(&global);
            elai_env_fill_missing_from_file(&global);
        }
    }

    // 2. Project-level .env (walks up from cwd, up to 12 levels)
    if let Ok(cwd) = env::current_dir() {
        for dir in cwd.ancestors().take(12) {
            let path = dir.join(".env");
            if path.is_file() {
                let _ = dotenvy::from_path(&path);
                elai_env_fill_missing_from_file(&path);
                return;
            }
        }
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

/// Idiomas suportados pelo Elai. Mantém em sincronia com `rust/locales/*.json`.
const SUPPORTED_LOCALES: &[&str] = &["pt-BR", "en"];

/// Carrega o locale do `~/.elai/config.json` e configura o `rust-i18n`.
///
/// Idiomas válidos: ver [`SUPPORTED_LOCALES`]. Locale ausente ou inválido cai
/// em `pt-BR` silenciosamente (sem panic). Não consulta `LANG`/`LC_*` por
/// decisão de design — locale é controlado apenas via config + slash command
/// `/locale`.
fn init_locale() {
    let cfg = runtime::global_config::load().unwrap_or_default();
    let locale = if SUPPORTED_LOCALES.contains(&cfg.locale.as_str()) {
        cfg.locale.as_str()
    } else {
        "pt-BR"
    };
    rust_i18n::set_locale(locale);
}

/// Handler do slash command `/locale [<idioma>]`.
///
/// - Sem argumento: retorna locale atual + lista de idiomas disponíveis.
/// - Com argumento válido: troca locale em runtime via `rust_i18n::set_locale`,
///   persiste em `~/.elai/config.json`, e confirma na nova língua.
/// - Com argumento inválido: mensagem de erro listando idiomas válidos. Sem
///   alteração de estado.
///
/// Retorna a mensagem composta (uma `String` multilinha). O caller decide se
/// printa em stdout (`LiveCli` non-TUI) ou empurra como `SystemNote` (TUI).
fn handle_locale_command(lang: Option<&str>) -> String {
    use std::fmt::Write as _;

    match lang {
        None => {
            let mut out = String::new();
            let current = rust_i18n::locale().to_string();
            let _ = writeln!(out, "{}", rust_i18n::t!("locale.current", lang = current));
            let _ = writeln!(out, "{}", rust_i18n::t!("locale.available_header"));
            for locale in SUPPORTED_LOCALES {
                let _ = writeln!(out, "  - {locale}");
            }
            out.trim_end().to_string()
        }
        Some(lang) if SUPPORTED_LOCALES.contains(&lang) => {
            rust_i18n::set_locale(lang);
            let mut out = String::new();
            if let Err(e) = persist_locale(lang) {
                let _ = writeln!(out, "warning: {e}");
            }
            let _ = write!(
                out,
                "{}",
                rust_i18n::t!("locale.changed", lang = lang.to_string())
            );
            out
        }
        Some(_) => rust_i18n::t!("locale.invalid").to_string(),
    }
}

fn persist_locale(lang: &str) -> Result<(), String> {
    let mut cfg = runtime::global_config::load()
        .map_err(|e| format!("failed to load config: {e}"))?;
    cfg.locale = lang.to_string();
    runtime::global_config::save(&cfg).map_err(|e| format!("failed to persist locale: {e}"))
}

fn has_any_auth() -> bool {
    let env_keys = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "OPENAI_API_KEY", "XAI_API_KEY"];
    if env_keys
        .iter()
        .any(|k| std::env::var_os(k).map(|v| !v.is_empty()).unwrap_or(false))
    {
        return true;
    }
    runtime::load_auth_method()
        .map(|opt| opt.is_some())
        .unwrap_or(false)
}

/// Sets only known API keys from `path` when they are not already in the process environment.
fn elai_env_fill_missing_from_file(path: &Path) {
    const KEYS: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ELAI_DEFAULT_OPENAI_MODEL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "XAI_API_KEY",
        "XAI_BASE_URL",
    ];
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line
            .strip_prefix("export ")
            .unwrap_or(line)
            .trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !KEYS.contains(&key) {
            continue;
        }
        if env::var_os(key).is_some() {
            continue;
        }
        let mut value = value.trim();
        if value.len() >= 2 {
            let bytes = value.as_bytes();
            let quote = bytes[0];
            if (quote == b'"' || quote == b'\'') && bytes[bytes.len() - 1] == quote {
                value = &value[1..value.len() - 1];
            }
        }
        env::set_var(key, value);
    }
}

#[derive(Debug, Clone, PartialEq)]
enum CliAction {
    DumpManifests,
    BootstrapPlan,
    Agents {
        args: Option<String>,
    },
    Skills {
        args: Option<String>,
    },
    PrintSystemPrompt {
        cwd: PathBuf,
        date: String,
    },
    Version,
    ResumeSession {
        session_path: PathBuf,
        commands: Vec<String>,
    },
    Prompt {
        prompt: String,
        model: String,
        output_format: CliOutputFormat,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
    },
    Login(crate::args::LoginArgs),
    Logout,
    Auth {
        cmd: crate::args::AuthCmd,
    },
    Init(crate::args::InitArgs),
    Repl {
        model: String,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
        no_tui: bool,
        no_cache: bool,
        swd_level: crate::swd::SwdLevel,
        budget_config: Option<BudgetConfig>,
    },
    // prompt-mode formatting is only supported for non-interactive runs
    Help,
    Verify,
    Update,
    Uninstall,
    Stats {
        days: Option<u32>,
        by_model: bool,
        by_project: bool,
    },
    // -----------------------------------------------------------------------
    // Headless / agent-mode commands
    // -----------------------------------------------------------------------
    Send {
        message: String,
        wait: bool,
        json: bool,
        /// Orçamento explícito de tokens para thinking (None = default por modelo).
        thinking_budget: Option<u32>,
    },
    ChatShow {
        last: usize,
        json: bool,
    },
    ModelGet,
    ModelSet {
        model: String,
    },
    Reply {
        answer: String,
    },
    StatusCmd {
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliOutputFormat {
    Text,
    Json,
}

impl CliOutputFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unsupported value for --output-format: {other} (expected text or json)"
            )),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn parse_args(args: &[String]) -> Result<CliAction, String> {
    let mut model = suggested_default_model();
    let mut output_format = CliOutputFormat::Text;
    let mut permission_mode = default_permission_mode();
    let mut wants_version = false;
    let mut allowed_tool_values = Vec::new();
    let mut rest = Vec::new();
    let mut no_tui = false;
    let mut no_cache = false;
    let mut swd_level_arg = crate::swd::SwdLevel::default();
    let mut budget_max_tokens: Option<u64> = None;
    let mut budget_max_usd: Option<f64> = None;
    let mut budget_max_turns: Option<u32> = None;
    let mut no_budget = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--version" | "-V" => {
                wants_version = true;
                index += 1;
            }
            "--model" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --model".to_string())?;
                model = resolve_model_alias(value);
                index += 2;
            }
            flag if flag.starts_with("--model=") => {
                model = resolve_model_alias(&flag[8..]);
                index += 1;
            }
            "--output-format" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --output-format".to_string())?;
                output_format = CliOutputFormat::parse(value)?;
                index += 2;
            }
            "--permission-mode" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --permission-mode".to_string())?;
                permission_mode = parse_permission_mode_arg(value)?;
                index += 2;
            }
            flag if flag.starts_with("--output-format=") => {
                output_format = CliOutputFormat::parse(&flag[16..])?;
                index += 1;
            }
            flag if flag.starts_with("--permission-mode=") => {
                permission_mode = parse_permission_mode_arg(&flag[18..])?;
                index += 1;
            }
            "--dangerously-skip-permissions" => {
                permission_mode = PermissionMode::DangerFullAccess;
                index += 1;
            }
            "--no-tui" => {
                no_tui = true;
                index += 1;
            }
            "--no-cache" => {
                no_cache = true;
                index += 1;
            }
            "--orchestrate" => {
                env::set_var("ELAI_ORCHESTRATE", "1");
                index += 1;
            }
            "--swd" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --swd".to_string())?;
                swd_level_arg = crate::swd::SwdLevel::from_str(value)
                    .ok_or_else(|| format!("invalid --swd: {value}"))?;
                index += 2;
            }
            flag if flag.starts_with("--swd=") => {
                let value = &flag[6..];
                swd_level_arg = crate::swd::SwdLevel::from_str(value)
                    .ok_or_else(|| format!("invalid --swd: {value}"))?;
                index += 1;
            }
            "--budget-tokens" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --budget-tokens".to_string())?;
                budget_max_tokens = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --budget-tokens: {value}"))?,
                );
                index += 2;
            }
            flag if flag.starts_with("--budget-tokens=") => {
                let v = &flag[16..];
                budget_max_tokens = Some(
                    v.parse::<u64>()
                        .map_err(|_| format!("invalid --budget-tokens: {v}"))?,
                );
                index += 1;
            }
            "--budget-usd" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --budget-usd".to_string())?;
                budget_max_usd = Some(
                    value
                        .parse::<f64>()
                        .map_err(|_| format!("invalid --budget-usd: {value}"))?,
                );
                index += 2;
            }
            flag if flag.starts_with("--budget-usd=") => {
                let v = &flag[13..];
                budget_max_usd = Some(
                    v.parse::<f64>()
                        .map_err(|_| format!("invalid --budget-usd: {v}"))?,
                );
                index += 1;
            }
            "--budget-turns" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --budget-turns".to_string())?;
                budget_max_turns = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --budget-turns: {value}"))?,
                );
                index += 2;
            }
            "--no-budget" => {
                no_budget = true;
                index += 1;
            }
            "-p" => {
                // Elai Code compat: -p "prompt" = one-shot prompt
                let prompt = args[index + 1..].join(" ");
                if prompt.trim().is_empty() {
                    return Err("-p requires a prompt string".to_string());
                }
                return Ok(CliAction::Prompt {
                    prompt,
                    model: resolve_model_alias(&model),
                    output_format,
                    allowed_tools: normalize_allowed_tools(&allowed_tool_values)?,
                    permission_mode,
                });
            }
            "--print" => {
                // Elai Code compat: --print makes output non-interactive
                output_format = CliOutputFormat::Text;
                index += 1;
            }
            "--allowedTools" | "--allowed-tools" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --allowedTools".to_string())?;
                allowed_tool_values.push(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--allowedTools=") => {
                allowed_tool_values.push(flag[15..].to_string());
                index += 1;
            }
            flag if flag.starts_with("--allowed-tools=") => {
                allowed_tool_values.push(flag[16..].to_string());
                index += 1;
            }
            other => {
                rest.push(other.to_string());
                index += 1;
            }
        }
    }

    if wants_version {
        return Ok(CliAction::Version);
    }

    let allowed_tools = normalize_allowed_tools(&allowed_tool_values)?;

    let budget_config = if no_budget {
        None
    } else if budget_max_tokens.is_some() || budget_max_usd.is_some() || budget_max_turns.is_some()
    {
        Some(BudgetConfig {
            max_tokens: budget_max_tokens,
            max_turns: budget_max_turns,
            max_cost_usd: budget_max_usd,
            warn_at_pct: 80.0,
        })
    } else {
        None
    };

    if rest.is_empty() {
        return Ok(CliAction::Repl {
            model,
            allowed_tools,
            permission_mode,
            no_tui,
            no_cache,
            swd_level: swd_level_arg,
            budget_config,
        });
    }
    if matches!(rest.first().map(String::as_str), Some("--help" | "-h")) {
        return Ok(CliAction::Help);
    }
    if rest.first().map(String::as_str) == Some("--resume") {
        return parse_resume_args(&rest[1..]);
    }

    match rest[0].as_str() {
        "dump-manifests" => Ok(CliAction::DumpManifests),
        "bootstrap-plan" => Ok(CliAction::BootstrapPlan),
        "agents" => Ok(CliAction::Agents {
            args: join_optional_args(&rest[1..]),
        }),
        "skills" => Ok(CliAction::Skills {
            args: join_optional_args(&rest[1..]),
        }),
        "system-prompt" => parse_system_prompt_args(&rest[1..]),
        "login" => parse_login_args(&rest[1..]),
        "logout" => Ok(CliAction::Logout),
        "auth" => parse_auth_args(&rest[1..]),
        "init" => Ok(CliAction::Init(parse_init_args(&rest[1..]))),
        "update" => Ok(CliAction::Update),
        "uninstall" => Ok(CliAction::Uninstall),
        "verify" => Ok(CliAction::Verify),
        "stats" => {
            let mut days: Option<u32> = None;
            let mut by_model = false;
            let mut by_project = false;
            let mut idx = 1usize;
            while idx < rest.len() {
                match rest[idx].as_str() {
                    "--days" => {
                        if let Some(v) = rest.get(idx + 1) {
                            days = v.parse().ok();
                            idx += 2;
                        } else {
                            idx += 1;
                        }
                    }
                    s if s.starts_with("--days=") => {
                        days = s[7..].parse().ok();
                        idx += 1;
                    }
                    "--by-model" => { by_model = true; idx += 1; }
                    "--by-project" => { by_project = true; idx += 1; }
                    _ => { idx += 1; }
                }
            }
            Ok(CliAction::Stats { days, by_model, by_project })
        }
        "prompt" => {
            let prompt = rest[1..].join(" ");
            if prompt.trim().is_empty() {
                return Err("prompt subcommand requires a prompt string".to_string());
            }
            Ok(CliAction::Prompt {
                prompt,
                model,
                output_format,
                allowed_tools,
                permission_mode,
            })
        }
        "send" => parse_send_args(&rest[1..]),
        "chat" => parse_chat_args(&rest[1..]),
        "model" => parse_model_args(&rest[1..]),
        "reply" => {
            let answer = rest[1..].join(" ");
            Ok(CliAction::Reply { answer })
        }
        "status" => {
            let json = rest[1..].iter().any(|a| a == "--json");
            Ok(CliAction::StatusCmd { json })
        }
        other if other.starts_with('/') => parse_direct_slash_cli_action(&rest),
        _other => Ok(CliAction::Prompt {
            prompt: rest.join(" "),
            model,
            output_format,
            allowed_tools,
            permission_mode,
        }),
    }
}

fn join_optional_args(args: &[String]) -> Option<String> {
    let joined = args.join(" ");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn parse_direct_slash_cli_action(rest: &[String]) -> Result<CliAction, String> {
    let raw = rest.join(" ");
    match SlashCommand::parse(&raw) {
        Some(SlashCommand::Help) => Ok(CliAction::Help),
        Some(SlashCommand::Agents { args }) => Ok(CliAction::Agents { args }),
        Some(SlashCommand::Skills { args }) => Ok(CliAction::Skills { args }),
        Some(command) => Err(format!(
            "unsupported direct slash command outside the REPL: {command_name}",
            command_name = match command {
                SlashCommand::Unknown(name) => format!("/{name}"),
                _ => rest[0].clone(),
            }
        )),
        None => Err(format!("unknown subcommand: {}", rest[0])),
    }
}

fn parse_login_args(args: &[String]) -> Result<CliAction, String> {
    let mut login_args = crate::args::LoginArgs::default();
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--console" => { login_args.console = true; idx += 1; }
            "--claudeai" => { login_args.claudeai = true; idx += 1; }
            "--sso" => { login_args.sso = true; idx += 1; }
            "--api-key" => { login_args.api_key = true; idx += 1; }
            "--token" => { login_args.token = true; idx += 1; }
            "--use-bedrock" => { login_args.use_bedrock = true; idx += 1; }
            "--use-vertex" => { login_args.use_vertex = true; idx += 1; }
            "--use-foundry" => { login_args.use_foundry = true; idx += 1; }
            "--no-browser" => { login_args.no_browser = true; idx += 1; }
            "--stdin" => { login_args.stdin = true; idx += 1; }
            "--legacy-elai" => { login_args.legacy_elai = true; idx += 1; }
            "--import-claude-code" => { login_args.import_claude_code = true; idx += 1; }
            "--yes" => { idx += 1; } // accepted but no-op at this layer (auth.rs uses it)
            "--no" => { idx += 1; }
            "--email" => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| "missing value for --email".to_string())?;
                login_args.email = Some(value.clone());
                idx += 2;
            }
            flag if flag.starts_with("--email=") => {
                login_args.email = Some(flag[8..].to_string());
                idx += 1;
            }
            other => {
                return Err(format!("unknown login flag: {other}"));
            }
        }
    }
    Ok(CliAction::Login(login_args))
}

fn parse_auth_args(args: &[String]) -> Result<CliAction, String> {
    let subcommand = args.first().map(String::as_str).unwrap_or("");
    match subcommand {
        "status" => {
            let json = args.iter().any(|a| a == "--json");
            Ok(CliAction::Auth {
                cmd: crate::args::AuthCmd::Status { json },
            })
        }
        "list" => Ok(CliAction::Auth {
            cmd: crate::args::AuthCmd::List,
        }),
        "" => Err("auth requires a subcommand: status, list".to_string()),
        other => Err(format!("unknown auth subcommand: {other}")),
    }
}

fn parse_send_args(args: &[String]) -> Result<CliAction, String> {
    let mut wait = false;
    let mut json = false;
    let mut stdin = false;
    let mut thinking_budget: Option<u32> = None;
    let mut parts: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--wait" => { wait = true; idx += 1; }
            "--json" => { json = true; idx += 1; }
            "--stdin" | "-" => { stdin = true; idx += 1; }
            "--ultrathink" => { thinking_budget = Some(32_000); idx += 1; }
            "--thinking" => {
                idx += 1;
                if let Some(raw) = args.get(idx) {
                    thinking_budget = raw.parse::<u32>().ok();
                    idx += 1;
                }
            }
            other => { parts.push(other.to_string()); idx += 1; }
        }
    }
    let message = if stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(|e| e.to_string())?;
        buf.trim().to_string()
    } else {
        parts.join(" ")
    };
    if message.is_empty() {
        return Err("send requires a message; use 'elai send \"text\"' or 'echo text | elai send -'".into());
    }
    Ok(CliAction::Send { message, wait, json, thinking_budget })
}

fn parse_chat_args(args: &[String]) -> Result<CliAction, String> {
    let sub = args.first().map(String::as_str).unwrap_or("show");
    match sub {
        "show" | "" => {
            let mut last = 20usize;
            let mut json = false;
            let mut idx = 1;
            while idx < args.len() {
                match args[idx].as_str() {
                    "--json" => { json = true; idx += 1; }
                    s if s.starts_with("--last=") => { last = s[7..].parse().unwrap_or(last); idx += 1; }
                    "--last" => {
                        if let Some(v) = args.get(idx + 1) { last = v.parse().unwrap_or(last); idx += 2; } else { idx += 1; }
                    }
                    _ => { idx += 1; }
                }
            }
            Ok(CliAction::ChatShow { last, json })
        }
        other => Err(format!("unknown chat subcommand: {other}; expected: show")),
    }
}

fn parse_model_args(args: &[String]) -> Result<CliAction, String> {
    match args.first().map(String::as_str) {
        Some("get") | None => Ok(CliAction::ModelGet),
        Some("set") => {
            let model = args.get(1).cloned().unwrap_or_default();
            if model.is_empty() {
                return Err("model set requires a model name".into());
            }
            Ok(CliAction::ModelSet { model })
        }
        Some(other) => Err(format!("unknown model subcommand: {other}; expected: get, set")),
    }
}

fn normalize_allowed_tools(values: &[String]) -> Result<Option<AllowedToolSet>, String> {
    current_tool_registry()?.normalize_allowed_tools(values)
}

fn current_tool_registry() -> Result<GlobalToolRegistry, String> {
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader.load().map_err(|error| error.to_string())?;
    let plugin_manager = build_plugin_manager(&cwd, &loader, &runtime_config);
    let plugin_tools = plugin_manager
        .aggregated_tools()
        .map_err(|error| error.to_string())?;
    GlobalToolRegistry::with_plugin_tools(plugin_tools)
}

fn parse_permission_mode_arg(value: &str) -> Result<PermissionMode, String> {
    normalize_permission_mode(value)
        .ok_or_else(|| {
            format!(
                "unsupported permission mode '{value}'. Use read-only, workspace-write, or danger-full-access."
            )
        })
        .map(permission_mode_from_label)
}

/// Remove o binário, ~/.elai/ e as linhas do shell RC inseridas pelo instalador.
#[cfg(windows)]
fn schedule_windows_cleanup(
    bin: &std::path::Path,
    elai_dir: Option<&std::path::Path>,
) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    // CREATE_NO_WINDOW (0x0800_0000) | DETACHED_PROCESS (0x0000_0008)
    const FLAGS: u32 = 0x0800_0000 | 0x0000_0008;

    let bin_str = bin.display().to_string();
    let dir_clause = match elai_dir {
        Some(d) => format!(r#" & rmdir /s /q "{}""#, d.display()),
        None => String::new(),
    };
    // timeout aguarda o processo pai (este elai.exe) liberar o lock antes de
    // tentar deletar. /nobreak impede interrupção por tecla.
    let cleanup = format!(
        r#"timeout /t 2 /nobreak > nul & del /f /q "{bin_str}"{dir_clause}"#
    );

    Command::new("cmd")
        .args(["/c", &cleanup])
        .creation_flags(FLAGS)
        .spawn()
        .map(|_| ())
}

fn perform_uninstall() -> String {
    let mut log = Vec::<String>::new();
    let mut errors = Vec::<String>::new();

    // 1. Binário — usa current_exe() para encontrar onde está instalado de fato
    let bin = std::env::current_exe()
        .unwrap_or_else(|_| {
            let install_dir = std::env::var("ELAI_INSTALL_DIR")
                .unwrap_or_else(|_| "/usr/local/bin".into());
            std::path::PathBuf::from(install_dir).join("elai")
        });

    let elai_dir = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| std::path::PathBuf::from(home).join(".elai"));

    #[cfg(windows)]
    {
        // Windows mantém lock exclusivo no .exe em execução: remove_file() falha
        // com "Access is denied". Agendamos um cmd destacado que aguarda o
        // processo morrer e então apaga binário + diretório.
        match schedule_windows_cleanup(&bin, elai_dir.as_deref()) {
            Ok(()) => {
                log.push(format!("✅ Agendado para remoção: {}", bin.display()));
                if let Some(dir) = &elai_dir {
                    log.push(format!("✅ Agendado para remoção: {}", dir.display()));
                }
                log.push(
                    "ℹ Limpeza ocorre 2s após o Elai encerrar. Reabra o terminal em seguida.".into(),
                );
            }
            Err(e) => errors.push(format!("⚠ Falha ao agendar limpeza: {e}")),
        }
    }

    #[cfg(not(windows))]
    {
        match std::fs::remove_file(&bin) {
            Ok(_) => log.push(format!("✅ Removido: {}", bin.display())),
            Err(e) => errors.push(format!("⚠ {}: {e}", bin.display())),
        }

        // 2. Diretório ~/.elai/ (inclui ~/.elai/fastembed_cache, ~/.elai/tasks/, etc.)
        if let Some(dir) = &elai_dir {
            match std::fs::remove_dir_all(dir) {
                Ok(_) => log.push(format!("✅ Removido: {}", dir.display())),
                Err(e) => errors.push(format!("⚠ {}: {e}", dir.display())),
            }
        }

        // 2b. Caches legados do fastembed (criados antes da centralização em
        // ~/.elai/fastembed_cache). Best-effort — silencioso se ausente.
        let mut legacy_caches: Vec<std::path::PathBuf> = Vec::new();
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home = std::path::PathBuf::from(home);
            legacy_caches.push(home.join(".cache").join(".fastembed_cache"));
            legacy_caches.push(home.join(".fastembed_cache"));
        }
        if let Ok(cwd) = std::env::current_dir() {
            legacy_caches.push(cwd.join(".fastembed_cache"));
        }
        for path in legacy_caches {
            if path.is_dir() {
                match std::fs::remove_dir_all(&path) {
                    Ok(_) => log.push(format!("✅ Cache fastembed legado removido: {}", path.display())),
                    Err(e) => errors.push(format!("⚠ {}: {e}", path.display())),
                }
            }
        }
    }

    // 3. Linhas do shell RC (bloco marcado com "# elai-code api keys")
    let shell = std::env::var("SHELL").unwrap_or_default();
    let home_str = std::env::var("HOME").unwrap_or_default();
    let rc_path = if shell.contains("zsh") {
        format!("{home_str}/.zshrc")
    } else if shell.contains("fish") {
        format!("{home_str}/.config/fish/config.fish")
    } else if shell.contains("bash") {
        format!("{home_str}/.bashrc")
    } else if std::path::Path::new(&format!("{home_str}/.zshrc")).exists() {
        format!("{home_str}/.zshrc")
    } else {
        format!("{home_str}/.bashrc")
    };

    const MARKER: &str = "# elai-code api keys";
    if let Ok(content) = std::fs::read_to_string(&rc_path) {
        let mut out_lines: Vec<&str> = Vec::new();
        let mut skip = false;
        for line in content.lines() {
            if line == MARKER {
                skip = true;
                continue;
            }
            if skip && (line.starts_with("export ANTHROPIC_API_KEY") || line.starts_with("export OPENAI_API_KEY")) {
                continue;
            }
            skip = false;
            out_lines.push(line);
        }
        let new_content = out_lines.join("\n") + "\n";
        if new_content.trim() != content.trim() {
            match std::fs::write(&rc_path, &new_content) {
                Ok(_) => log.push(format!("✅ Linhas elai removidas de {rc_path}")),
                Err(e) => errors.push(format!("⚠ Não foi possível atualizar {rc_path}: {e}")),
            }
        }
    }

    let mut result = log.join("\n");
    if !errors.is_empty() {
        result.push_str("\n\nAvisos:\n");
        result.push_str(&errors.join("\n"));
    }
    result
}

fn estimate_tui_cost(app: &tui::UiApp) -> f64 {
    let (in_rate, out_rate) = if app.model.contains("gpt-4") {
        (0.000_005, 0.000_015)
    } else if app.model.contains("sonnet") {
        (0.000_003, 0.000_015)
    } else if app.model.contains("haiku") {
        (0.000_000_8, 0.000_004)
    } else {
        (0.000_015, 0.000_075)
    };
    f64::from(app.input_tokens) * in_rate + f64::from(app.output_tokens) * out_rate
}

fn permission_mode_from_label(mode: &str) -> PermissionMode {
    match mode {
        "read-only" => PermissionMode::ReadOnly,
        "workspace-write" => PermissionMode::WorkspaceWrite,
        "danger-full-access" => PermissionMode::DangerFullAccess,
        other => panic!("unsupported permission mode label: {other}"),
    }
}

fn default_permission_mode() -> PermissionMode {
    env::var("ELAI_PERMISSION_MODE")
        .ok()
        .as_deref()
        .and_then(normalize_permission_mode)
        .map_or(PermissionMode::DangerFullAccess, permission_mode_from_label)
}

fn filter_tool_specs(
    tool_registry: &GlobalToolRegistry,
    allowed_tools: Option<&AllowedToolSet>,
    catalog: &runtime::ToolCatalog,
    active_skill: Option<&runtime::Skill>,
) -> Vec<ToolDefinition> {
    use runtime::{run_pipeline, set_turn_snapshot, FilterPattern, PipelineTool, ToolBudgetConfig};

    // Build the list of all tools from the registry (no pre-filtering).
    let all_tools: Vec<PipelineTool> = tool_registry
        .definitions(None)
        .into_iter()
        .map(|def| PipelineTool { name: def.name })
        .collect();

    // Convert MatcherPattern → FilterPattern for the pipeline.
    let filter_patterns: Option<Vec<FilterPattern>> = allowed_tools.map(|patterns| {
        patterns
            .iter()
            .map(|p| match p {
                MatcherPattern::Exact(s) => FilterPattern::Exact(s.clone()),
                MatcherPattern::Prefix(s) => FilterPattern::Prefix(s.clone()),
            })
            .collect()
    });

    let result = run_pipeline(
        all_tools,
        catalog,
        filter_patterns.as_deref(),
        active_skill,
        &ToolBudgetConfig::default(),
    );

    // Persist snapshot for `/tools why`.
    set_turn_snapshot(result.clone());

    // Reconstruct ordered ToolDefinition list using the accepted names.
    let all_defs: std::collections::HashMap<String, ToolDefinition> = tool_registry
        .definitions(None)
        .into_iter()
        .map(|def| (def.name.clone(), def))
        .collect();

    result
        .tool_names
        .into_iter()
        .filter_map(|name| all_defs.get(&name).cloned())
        .collect()
}

fn parse_system_prompt_args(args: &[String]) -> Result<CliAction, String> {
    let mut cwd = env::current_dir().map_err(|error| error.to_string())?;
    let mut date = DEFAULT_DATE.to_string();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--cwd" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --cwd".to_string())?;
                cwd = PathBuf::from(value);
                index += 2;
            }
            "--date" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --date".to_string())?;
                date.clone_from(value);
                index += 2;
            }
            other => return Err(format!("unknown system-prompt option: {other}")),
        }
    }

    Ok(CliAction::PrintSystemPrompt { cwd, date })
}

fn parse_resume_args(args: &[String]) -> Result<CliAction, String> {
    let session_path = args
        .first()
        .ok_or_else(|| "missing session path for --resume".to_string())
        .map(PathBuf::from)?;
    let commands = args[1..].to_vec();
    if commands
        .iter()
        .any(|command| !command.trim_start().starts_with('/'))
    {
        return Err("--resume trailing arguments must be slash commands".to_string());
    }
    Ok(CliAction::ResumeSession {
        session_path,
        commands,
    })
}

fn run_stats_command(days: Option<u32>, by_model: bool, by_project: bool) {
    use commands::stats::render_stats_report;
    use runtime::{default_telemetry_path, load_entries};
    use std::time::{SystemTime, UNIX_EPOCH};

    let path = default_telemetry_path();
    let since_secs: Option<u64> = days.map(|d| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(u64::from(d) * 86400)
    });
    let entries = match load_entries(&path, since_secs) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error reading telemetry: {e}");
            return;
        }
    };
    print!("{}", render_stats_report(&entries, by_model, by_project, days));
}

// ---------------------------------------------------------------------------
// Headless / agent-mode handlers
// ---------------------------------------------------------------------------

/// `elai send "message"` — fire a single prompt and print the response to stdout.
/// With `--wait` it waits for the full response (default); always waits currently.
/// With `--json` it emits JSON (same structure as CliAction::Prompt with --output-format=json).
/// With `--thinking N` ou `--ultrathink` configura o orçamento de extended thinking.
/// Quando thinking está ativo (override explícito, palavra-chave `ultrathink` no
/// texto, ou default por modelo), o `CAPYBARA_SYSTEM_PROMPT` é anexado ao system
/// prompt para reforçar a disciplina de raciocínio profundo.
fn run_headless_send(
    message: &str,
    _wait: bool,
    json: bool,
    thinking_budget: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let model = suggested_default_model();
    let output_format = if json { CliOutputFormat::Json } else { CliOutputFormat::Text };
    let allowed_tools = None;
    let permission_mode = default_permission_mode();

    // Detecta ultrathink no texto OU --ultrathink/--thinking flag.
    let ultrathink_keyword = message.to_ascii_lowercase().contains("ultrathink");
    let thinking_config: Option<ThinkingConfig> = match thinking_budget {
        Some(budget) => Some(ThinkingConfig::Enabled { budget_tokens: budget }),
        None if ultrathink_keyword => Some(ThinkingConfig::Enabled { budget_tokens: 32_000 }),
        None => default_thinking_config(&model),
    };

    let extra_system = if thinking_config.is_some() {
        Some(runtime::CAPYBARA_SYSTEM_PROMPT.to_string())
    } else {
        None
    };

    LiveCli::new_with_thinking(
        model,
        true,
        allowed_tools,
        permission_mode,
        false,
        thinking_config,
        extra_system,
    )?
    .run_turn_with_output(message, output_format)
}

/// `elai chat show [--last N] [--json]` — show the last N messages from the most-recent session.
fn run_chat_show(last: usize, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let sessions_path = sessions_dir()?;
    // Find most-recently-modified session file
    let mut entries: Vec<_> = fs::read_dir(&sessions_path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    entries.reverse();
    let session_path = match entries.first() {
        Some(entry) => entry.path(),
        None => {
            println!("No sessions found.");
            return Ok(());
        }
    };
    let session = runtime::Session::load_from_path(&session_path)
        .map_err(|e| format!("could not load session: {e}"))?;
    let messages = &session.messages;
    let start = messages.len().saturating_sub(last);
    let slice = &messages[start..];
    if json {
        let arr: Vec<serde_json::Value> = slice.iter().map(|msg| {
            let role = format!("{:?}", msg.role).to_lowercase();
            let text: Vec<String> = msg.blocks.iter().filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }).collect();
            serde_json::json!({ "role": role, "text": text.join("\n") })
        }).collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else {
        println!("Session: {}", session_path.display());
        println!("Messages: {} (showing last {})", messages.len(), slice.len());
        println!("{}", "─".repeat(60));
        for msg in slice {
            let role = format!("{:?}", msg.role);
            let text: String = msg.blocks.iter().filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join("\n");
            println!("[{role}] {}", text.lines().take(5).collect::<Vec<_>>().join(" ↵ "));
        }
    }
    Ok(())
}

/// `elai model get` — print the current resolved default model.
fn run_model_get() -> Result<(), Box<dyn std::error::Error>> {
    let model = suggested_default_model();
    // Check for a persisted override
    let override_model: Option<String> = std::env::var("ELAI_DEFAULT_OPENAI_MODEL").ok()
        .filter(|v| !v.trim().is_empty());
    if let Some(ref ov) = override_model {
        println!("{ov}");
    } else {
        println!("{model}");
    }
    Ok(())
}

/// `elai model set MODEL` — persist the preferred model to `~/.elai/.env`.
fn run_model_set(model: &str) -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .map_err(|_| "HOME is not set")?;
    let env_path = home.join(".elai").join(".env");
    std::fs::create_dir_all(env_path.parent().unwrap())?;
    // Read existing content, replace or append ELAI_DEFAULT_OPENAI_MODEL
    let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
    let key = "ELAI_DEFAULT_OPENAI_MODEL";
    let new_line = format!("{key}={model}");
    let updated: String = if existing.lines().any(|l| l.starts_with(&format!("{key}="))) {
        existing.lines()
            .map(|l| if l.starts_with(&format!("{key}=")) { new_line.as_str() } else { l })
            .collect::<Vec<_>>()
            .join("\n") + "\n"
    } else {
        if existing.is_empty() || existing.ends_with('\n') {
            format!("{existing}{new_line}\n")
        } else {
            format!("{existing}\n{new_line}\n")
        }
    };
    std::fs::write(&env_path, &updated)?;
    println!("Model set to: {model}");
    println!("Saved to: {}", env_path.display());
    Ok(())
}

/// `elai reply "answer"` — send a reply in the context of the most recent session.
/// Implemented as a headless send (the session continuity is handled by LiveCli resumption).
fn run_reply(answer: &str) -> Result<(), Box<dyn std::error::Error>> {
    run_headless_send(answer, true, false, None)
}

/// `elai status [--json]` — show elai version, current model, auth state, and cwd.
fn run_status_cmd(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let version = env!("CARGO_PKG_VERSION");
    let model = suggested_default_model();
    let auth_info = auth::dispatch_auth_status(false).map_or_else(
        |_| "unknown".to_string(),
        |_| String::new(), // dispatch_auth_status prints itself when json=false
    );
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let _ = auth_info;
    if json {
        // Build JSON manually; auth info comes from collect_auth_info via dispatch
        let has_auth = has_any_auth();
        println!("{}", serde_json::json!({
            "version": version,
            "model": model,
            "has_auth": has_auth,
            "cwd": cwd,
        }));
    } else {
        println!("elai {version}");
        println!("model  : {model}");
        println!("cwd    : {cwd}");
        println!("auth   :");
        let _ = auth::dispatch_auth_status(false);
    }
    Ok(())
}

fn run_verify_command() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let output = verify::run_verify(&cwd)?;
    println!("{output}");
    Ok(())
}

fn start_telemetry() -> (TelemetryHandle, Option<TelemetryShutdown>) {
    if std::env::var("ELAI_TELEMETRY").as_deref() == Ok("off") {
        return (TelemetryHandle::noop(), None);
    }
    let (handle, shutdown) = TelemetryWorker::start();
    (handle, Some(shutdown))
}

fn dump_manifests() {
    let workspace_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let paths = UpstreamPaths::from_workspace_dir(&workspace_dir);
    match extract_manifest(&paths) {
        Ok(manifest) => {
            println!("commands: {}", manifest.commands.entries().len());
            println!("tools: {}", manifest.tools.entries().len());
            println!("bootstrap phases: {}", manifest.bootstrap.phases().len());
        }
        Err(error) => {
            eprintln!("failed to extract manifests: {error}");
            std::process::exit(1);
        }
    }
}

fn print_bootstrap_plan() {
    for phase in runtime::BootstrapPlan::elai_default().phases() {
        println!("- {phase:?}");
    }
}


fn print_system_prompt(cwd: PathBuf, date: String) {
    match load_system_prompt(cwd, date, env::consts::OS, "unknown") {
        Ok(sections) => println!("{}", sections.join("\n\n")),
        Err(error) => {
            eprintln!("failed to build system prompt: {error}");
            std::process::exit(1);
        }
    }
}

fn print_version() {
    println!("{}", render_version_report());
}

fn resume_session(session_path: &Path, commands: &[String]) {
    let session = match Session::load_from_path(session_path) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("failed to restore session: {error}");
            std::process::exit(1);
        }
    };

    if commands.is_empty() {
        println!(
            "Restored session from {} ({} messages).",
            session_path.display(),
            session.messages.len()
        );
        return;
    }

    let mut session = session;
    for raw_command in commands {
        let Some(command) = SlashCommand::parse(raw_command) else {
            eprintln!("unsupported resumed command: {raw_command}");
            std::process::exit(2);
        };
        match run_resume_command(session_path, &session, &command) {
            Ok(ResumeCommandOutcome {
                session: next_session,
                message,
            }) => {
                session = next_session;
                if let Some(message) = message {
                    println!("{message}");
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ResumeCommandOutcome {
    session: Session,
    message: Option<String>,
}

#[derive(Debug, Clone)]
struct StatusContext {
    cwd: PathBuf,
    session_path: Option<PathBuf>,
    loaded_config_files: usize,
    discovered_config_files: usize,
    memory_file_count: usize,
    project_root: Option<PathBuf>,
    git_branch: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct StatusUsage {
    message_count: usize,
    turns: u32,
    latest: TokenUsage,
    cumulative: TokenUsage,
    estimated_tokens: usize,
}

fn format_model_report(model: &str, message_count: usize, turns: u32) -> String {
    format!(
        "Model
  Current model    {model}
  Session messages {message_count}
  Session turns    {turns}

Usage
  Inspect current model with /model
  Switch models with /model <name>"
    )
}

fn format_model_switch_report(previous: &str, next: &str, message_count: usize) -> String {
    format!(
        "Model updated
  Previous         {previous}
  Current          {next}
  Preserved msgs   {message_count}"
    )
}

fn format_permissions_report(mode: &str) -> String {
    let modes = [
        ("read-only", "Read/search tools only", mode == "read-only"),
        (
            "workspace-write",
            "Edit files inside the workspace",
            mode == "workspace-write",
        ),
        (
            "danger-full-access",
            "Unrestricted tool access",
            mode == "danger-full-access",
        ),
    ]
    .into_iter()
    .map(|(name, description, is_current)| {
        let marker = if is_current {
            "● current"
        } else {
            "○ available"
        };
        format!("  {name:<18} {marker:<11} {description}")
    })
    .collect::<Vec<_>>()
    .join(
        "
",
    );

    format!(
        "Permissions
  Active mode      {mode}
  Mode status      live session default

Modes
{modes}

Usage
  Inspect current mode with /permissions
  Switch modes with /permissions <mode>"
    )
}

fn format_permissions_switch_report(previous: &str, next: &str) -> String {
    format!(
        "Permissions updated
  Result           mode switched
  Previous mode    {previous}
  Active mode      {next}
  Applies to       subsequent tool calls
  Usage            /permissions to inspect current mode"
    )
}

fn format_cost_report(usage: TokenUsage) -> String {
    format!(
        "Cost
  Input tokens     {}
  Output tokens    {}
  Cache create     {}
  Cache read       {}
  Total tokens     {}",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
        usage.total_tokens(),
    )
}

fn format_resume_report(session_path: &str, message_count: usize, turns: u32) -> String {
    format!(
        "Session resumed
  Session file     {session_path}
  Messages         {message_count}
  Turns            {turns}"
    )
}

fn format_compact_report(removed: usize, resulting_messages: usize, skipped: bool) -> String {
    if skipped {
        format!(
            "Compact
  Result           skipped
  Reason           session below compaction threshold
  Messages kept    {resulting_messages}"
        )
    } else {
        format!(
            "Compact
  Result           compacted
  Messages removed {removed}
  Messages kept    {resulting_messages}"
        )
    }
}

fn parse_git_status_metadata(status: Option<&str>) -> (Option<PathBuf>, Option<String>) {
    let Some(status) = status else {
        return (None, None);
    };
    let branch = status.lines().next().and_then(|line| {
        line.strip_prefix("## ")
            .map(|line| {
                line.split(['.', ' '])
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .filter(|value| !value.is_empty())
    });
    let project_root = find_git_root().ok();
    (project_root, branch)
}

fn find_git_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(env::current_dir()?)
        .output()?;
    if !output.status.success() {
        return Err("not a git repository".into());
    }
    let path = String::from_utf8(output.stdout)?.trim().to_string();
    if path.is_empty() {
        return Err("empty git root".into());
    }
    Ok(PathBuf::from(path))
}

#[allow(clippy::too_many_lines)]
fn run_resume_command(
    session_path: &Path,
    session: &Session,
    command: &SlashCommand,
) -> Result<ResumeCommandOutcome, Box<dyn std::error::Error>> {
    match command {
        SlashCommand::Help => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_repl_help()),
        }),
        SlashCommand::Compact => {
            let result = runtime::compact_session(
                session,
                CompactionConfig {
                    max_estimated_tokens: 0,
                    ..CompactionConfig::default()
                },
            );
            let removed = result.removed_message_count;
            let kept = result.compacted_session.messages.len();
            let skipped = removed == 0;
            result.compacted_session.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: result.compacted_session,
                message: Some(format_compact_report(removed, kept, skipped)),
            })
        }
        SlashCommand::Clear { confirm } => {
            if !confirm {
                return Ok(ResumeCommandOutcome {
                    session: session.clone(),
                    message: Some(
                        "clear: confirmation required; rerun with /clear --confirm".to_string(),
                    ),
                });
            }
            let cleared = Session::new();
            cleared.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: cleared,
                message: Some(format!(
                    "Cleared resumed session file {}.",
                    session_path.display()
                )),
            })
        }
        SlashCommand::Status => {
            let tracker = UsageTracker::from_session(session);
            let usage = tracker.cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_status_report(
                    "restored-session",
                    StatusUsage {
                        message_count: session.messages.len(),
                        turns: tracker.turns(),
                        latest: tracker.current_turn_usage(),
                        cumulative: usage,
                        estimated_tokens: 0,
                    },
                    default_permission_mode().as_str(),
                    &status_context(Some(session_path))?,
                )),
            })
        }
        SlashCommand::Cost => {
            let usage = UsageTracker::from_session(session).cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_cost_report(usage)),
            })
        }
        SlashCommand::Config { section } => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_config_report(section.as_deref())?),
        }),
        SlashCommand::Memory => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_memory_report()?),
        }),
        SlashCommand::Init => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(init_elai_md()?),
        }),
        SlashCommand::Diff => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_diff_report()?),
        }),
        SlashCommand::Version => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_version_report()),
        }),
        SlashCommand::Export { path } => {
            let export_path = resolve_export_path(path.as_deref(), session)?;
            fs::write(&export_path, render_export_text(session))?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format!(
                    "Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}",
                    export_path.display(),
                    session.messages.len(),
                )),
            })
        }
        SlashCommand::Agents { args } => {
            let cwd = env::current_dir()?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(handle_agents_slash_command(args.as_deref(), &cwd)?),
            })
        }
        SlashCommand::Skills { args } => {
            let cwd = env::current_dir()?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(handle_skills_slash_command(args.as_deref(), &cwd)?),
            })
        }
        SlashCommand::Bughunter { .. }
        | SlashCommand::Branch { .. }
        | SlashCommand::Worktree { .. }
        | SlashCommand::CommitPushPr { .. }
        | SlashCommand::Commit
        | SlashCommand::Pr { .. }
        | SlashCommand::Issue { .. }
        | SlashCommand::Ultraplan { .. }
        | SlashCommand::Teleport { .. }
        | SlashCommand::DebugToolCall
        | SlashCommand::Resume { .. }
        | SlashCommand::Model { .. }
        | SlashCommand::Permissions { .. }
        | SlashCommand::Session { .. }
        | SlashCommand::Plugins { .. }
        | SlashCommand::Budget { .. }
        | SlashCommand::Tools { .. }
        | SlashCommand::Dream { .. }
        | SlashCommand::Stats { .. }
        | SlashCommand::Providers { .. }
        | SlashCommand::Cache { .. }
        | SlashCommand::Verify
        | SlashCommand::Locale { .. }
        | SlashCommand::Update
        | SlashCommand::Unknown(_) => Err("unsupported resumed slash command".into()),
    }
}

fn run_repl(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    no_tui: bool,
    no_cache: bool,
    swd_level: crate::swd::SwdLevel,
    budget_config: Option<BudgetConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Use TUI mode when stdout is a real terminal and --no-tui not specified.
    if !no_tui && std::io::stdout().is_terminal() {
        return run_tui_repl(model, allowed_tools, permission_mode, swd_level, budget_config);
    }
    let _ = budget_config;

    // Fallback: plain text REPL (piped/non-TTY).
    let mut cli = LiveCli::new(model, true, allowed_tools, permission_mode, no_cache)?;
    let mut editor = input::LineEditor::new("> ", slash_command_completion_candidates());
    println!("{}", cli.startup_banner());

    loop {
        match editor.read_line()? {
            input::ReadOutcome::Submit(input) => {
                let trimmed = input.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if matches!(trimmed.as_str(), "/exit" | "/quit") {
                    cli.persist_session()?;
                    break;
                }
                if let Some(command) = SlashCommand::parse(&trimmed) {
                    // Before dispatching Unknown to handle_repl_command, try user commands.
                    if matches!(command, SlashCommand::Unknown(_)) {
                        let cwd = std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
                        if let Some(expanded) =
                            try_user_command(&trimmed, &cli.user_commands, &cwd)
                        {
                            eprintln!(
                                "[custom] /{} expanded ({} chars)",
                                expanded.command_name,
                                expanded.expanded_prompt.len()
                            );
                            editor.push_history(input);
                            cli.run_turn(&expanded.expanded_prompt)?;
                            continue;
                        }
                    }
                    if cli.handle_repl_command(command)? {
                        cli.persist_session()?;
                    }
                    continue;
                }
                editor.push_history(input);
                cli.run_turn(&trimmed)?;
            }
            input::ReadOutcome::Cancel => {}
            input::ReadOutcome::Exit => {
                cli.persist_session()?;
                break;
            }
        }
    }

    Ok(())
}

fn run_tui_repl(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    swd_level: crate::swd::SwdLevel,
    budget_config: Option<BudgetConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::AtomicU8;
    use std::sync::Arc;

    // Channels: TUI msg (runtime → TUI) and perm request/decision.
    let (msg_tx, msg_rx) = mpsc::channel::<tui::TuiMsg>();
    let (perm_tx, perm_rx) = mpsc::channel::<tui::PermRequest>();

    // Substitui o sink default global do `runtime` por um que envia
    // `TuiMsg::TaskProgress` no canal acima. Qualquer `with_task_default(...)`
    // disparado por dentro do TUI passa a renderizar como `ChatEntry::TaskProgress`
    // in-place (uma só linha viva por task).
    runtime::set_default_sink(Arc::new(tui_sink::ChannelSink::new(msg_tx.clone())));

    // Load recent sessions for the side panel.
    let recent_sessions: Vec<(String, usize)> = list_managed_sessions()
        .unwrap_or_default()
        .into_iter()
        .take(5)
        .map(|s| (s.id, s.message_count))
        .collect();

    let session_handle = create_managed_session_handle()?;
    let system_prompt = build_system_prompt()?;

    let swd_atomic = Arc::new(AtomicU8::new(swd_level as u8));

    // Resolve budget tracker: CLI flags → .elai/budget.json → disabled
    let effective_budget = if let Some(cfg) = budget_config {
        BudgetTracker::new(cfg)
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(cfg) = load_budget_config(&cwd) {
            BudgetTracker::new(cfg)
        } else {
            BudgetTracker::disabled()
        }
    };
    let budget_tracker = std::sync::Arc::new(std::sync::Mutex::new(effective_budget));

    let mut app = tui::UiApp::new(
        model.clone(),
        permission_mode.as_str().to_string(),
        session_handle.id.clone(),
        recent_sessions,
        Arc::clone(&swd_atomic),
    );
    app.budget_enabled = budget_tracker.lock().unwrap().is_enabled();

    // Background update check — first run at startup, then every hour while the
    // TUI is alive. Results surface as a SystemNote; never blocks or forces a
    // terminal-mode prompt. Each new latest version is announced at most once
    // per session to avoid spamming long-running sessions.
    {
        let update_tx = msg_tx.clone();
        let interval_secs: u64 = std::env::var("ELAI_UPDATE_CHECK_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);
        std::thread::spawn(move || {
            let interval = std::time::Duration::from_secs(interval_secs);
            let mut last_notified: Option<String> = None;
            loop {
                if let Some(upd) = updater::check_available() {
                    if last_notified.as_deref() != Some(upd.latest.as_str()) {
                        if update_tx
                            .send(tui::TuiMsg::SystemNote(format!(
                                "⬆ Nova versão disponível: v{} → v{}. Digite /update para atualizar.",
                                upd.current, upd.latest
                            )))
                            .is_err()
                        {
                            break;
                        }
                        last_notified = Some(upd.latest);
                    }
                }
                std::thread::sleep(interval);
            }
        });
    }

    // First-run: open setup wizard; otherwise only open auth picker if no auth present.
    if !runtime::is_setup_complete() {
        app.open_first_run_wizard();
    } else if !has_any_auth() {
        app.open_auth_picker();
    }

    // Tracks the last warning threshold already shown (90 or 80) to avoid repeating.
    let mut budget_warned_at: u8 = 0;

    // Install a panic hook that restores the terminal before printing the panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture
        );
        original_hook(info);
    }));

    // Set up terminal.
    let mut stdout = io::stdout();
    tui::enter_tui(&mut stdout)?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    // State shared between TUI loop and runtime thread: the active session.
    let session = Arc::new(std::sync::Mutex::new(Session::new()));

    // Track whether the runtime thread is currently running.
    let (thread_done_tx, thread_done_rx) = mpsc::channel::<Result<(), String>>();

    // Main TUI loop.
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            tui::render(&mut terminal, &mut app)?;
            app.tick();
            tui::drain_auth_events(&mut app);

            let action = tui::poll_and_handle(&mut app, &msg_rx, &perm_rx);

            // Drain Done/Error from background thread.
            if let Ok(outcome) = thread_done_rx.try_recv() {
                app.thinking = false;
                if let Err(e) = outcome {
                    app.push_chat(tui::ChatEntry::SystemNote(format!("❌ {e}")));
                }
                // Persist session.
                {
                    let guard = session.lock().unwrap();
                    let _ = guard.save_to_path(&session_handle.path);
                }
                // Budget check after each turn
                {
                    let usage = {
                        let guard = session.lock().unwrap();
                        UsageTracker::from_session(&guard)
                    };
                    let bt = budget_tracker.lock().unwrap();
                    if bt.is_enabled() {
                        let pct_data = bt.usage_pct(&usage, &app.model);
                        app.budget_pct = pct_data.highest_pct;
                        app.budget_cost_usd = pct_data.current_cost_usd;
                        match bt.check(&usage, &app.model) {
                            BudgetStatus::Exhausted { reason } => {
                                let _ = append_budget_summary_to_memory(
                                    &app.model,
                                    &usage,
                                    &pct_data,
                                    &reason,
                                );
                                app.push_chat(tui::ChatEntry::SystemNote(format!(
                                    "🛑 Budget esgotado: {reason}\n💡 Aumente com --budget-tokens N ou /budget N"
                                )));
                            }
                            BudgetStatus::Warning { pct, dimension } => {
                                let threshold = if pct >= 90.0 { 90u8 } else { 80u8 };
                                if budget_warned_at < threshold {
                                    budget_warned_at = threshold;
                                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                                        "⚠️  Budget {pct:.0}% consumido ({dimension})"
                                    )));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            match action {
                tui::TuiAction::Quit => {
                    app.should_quit = true;
                }
                tui::TuiAction::SendMessage(text) => {
                    let ultrathink = text.to_ascii_lowercase().contains("ultrathink");
                    if !app.thinking {
                        budget_warned_at = 0;
                        app.thinking = true;
                        let model_clone = app.model.clone();
                        let perm_clone = permission_mode_from_label(
                            &app.permission_mode
                        );
                        let allowed_clone = allowed_tools.clone();
                        let mut prompt_clone = system_prompt.clone();
                        // Inject SWD full-mode system prompt if enabled at spawn time.
                        {
                            use std::sync::atomic::Ordering;
                            if crate::swd::SwdLevel::from_u8(swd_atomic.load(Ordering::Relaxed))
                                == crate::swd::SwdLevel::Full
                            {
                                prompt_clone.push(crate::swd::SWD_FULL_SYSTEM_PROMPT.to_string());
                            }
                        }
                        // Injeta CAPYBARA quando ultrathink está ativo.
                        if ultrathink {
                            prompt_clone.push(runtime::CAPYBARA_SYSTEM_PROMPT.to_string());
                        }
                        let session_clone = {
                            let guard = session.lock().unwrap();
                            guard.clone()
                        };
                        let msg_tx_clone = msg_tx.clone();
                        let perm_tx_clone = perm_tx.clone();
                        let done_tx = thread_done_tx.clone();
                        let session_for_thread = Arc::clone(&session);
                        let swd_atomic_clone = Arc::clone(&swd_atomic);
                        let thinking_override: Option<ThinkingConfig> = if ultrathink {
                            Some(ThinkingConfig::Enabled { budget_tokens: 32_000 })
                        } else {
                            None
                        };

                        thread::spawn(move || {
                            let result: Result<(), String> = (|| {
                                let mut runtime = build_runtime_for_tui(
                                    session_clone,
                                    model_clone.clone(),
                                    prompt_clone,
                                    allowed_clone,
                                    perm_clone,
                                    msg_tx_clone.clone(),
                                    swd_atomic_clone,
                                    thinking_override,
                                ).map_err(|e| {
                                    let msg = e.to_string();
                                    let _ = msg_tx_clone.send(tui::TuiMsg::Error(msg.clone()));
                                    msg
                                })?;
                                let mut prompter = CliPermissionPrompter::new_tui(
                                    perm_clone,
                                    perm_tx_clone,
                                );
                                let turn_result = runtime.run_turn(&text, Some(&mut prompter));
                                // Persist whatever the runtime produced — even on error — so the
                                // user's input and any partial assistant work are not lost. Without
                                // this, a mid-turn failure rewinds the session to the pre-turn
                                // snapshot and the next turn looks like it "forgot" the context.
                                let _ = session_for_thread
                                    .lock()
                                    .map(|mut guard| *guard = runtime.session().clone());
                                if let Err(e) = turn_result {
                                    let msg = e.to_string();
                                    let _ = msg_tx_clone.send(tui::TuiMsg::Error(msg.clone()));
                                    return Err(msg);
                                }
                                let _ = msg_tx_clone.send(tui::TuiMsg::Done);
                                Ok(())
                            })();
                            let _ = done_tx.send(result);
                        });
                    }
                }
                tui::TuiAction::SetModel(m) => {
                    app.model = m.clone();
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "✅ Modelo alterado para: {m}"
                    )));
                }
                tui::TuiAction::SetPermissions(p) => {
                    app.permission_mode = p.clone();
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "✅ Permissões alteradas para: {p}"
                    )));
                }
                tui::TuiAction::ResumeSession(session_id) => {
                    if let Ok(handle) = resolve_session_reference(&session_id) {
                        if let Ok(loaded) = Session::load_from_path(&handle.path) {
                            let msg_count = loaded.messages.len();
                            sync_session_to_app_chat(&loaded, &mut app);
                            *session.lock().unwrap() = loaded;
                            app.push_chat(tui::ChatEntry::SystemNote(format!(
                                "✅ Sessão {session_id} retomada ({msg_count} mensagens)"
                            )));
                        }
                    }
                }
                tui::TuiAction::SlashCommand(cmd) => {
                    handle_tui_slash_command(cmd, &mut app, &session, &budget_tracker, &msg_tx);
                }
                tui::TuiAction::EnterReadMode => {
                    app.read_mode = true;
                    let _ = crossterm::execute!(
                        terminal.backend_mut(),
                        DisableMouseCapture
                    );
                }
                tui::TuiAction::ExitReadMode => {
                    app.read_mode = false;
                    let _ = crossterm::execute!(
                        terminal.backend_mut(),
                        EnableMouseCapture
                    );
                }
                tui::TuiAction::SetupComplete => {
                    // Re-detect the best model now that keys are in the environment.
                    let new_model = suggested_default_model();
                    app.model = new_model.clone();
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "\u{2705} API key salva em ~/.elai/.env\n  Modelo padrão: {new_model}"
                    )));
                }
                tui::TuiAction::AuthComplete { label } => {
                    let new_model = suggested_default_model();
                    app.model = new_model.clone();
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "\u{2705} {label}\n  Modelo padrão: {new_model}"
                    )));
                }
                tui::TuiAction::Uninstall => {
                    let report = perform_uninstall();
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "Desinstalação concluída:\n{report}\n\nEla Code foi removido. Encerrando..."
                    )));
                    app.should_quit = true;
                }
                tui::TuiAction::None => {}
            }

            if app.should_quit {
                break;
            }
        }
        Ok(())
    })();

    // Restore terminal — drop terminal first so it releases stdout, then leave TUI.
    drop(terminal);
    let _ = tui::leave_tui(&mut io::stdout());

    result
}

fn append_budget_summary_to_memory(
    model: &str,
    usage: &UsageTracker,
    pct: &BudgetUsagePct,
    reason: &str,
) -> std::io::Result<()> {
    use std::io::Write;
    let cwd = std::env::current_dir().unwrap_or_default();
    let memory_path = if cwd.join("MEMORY.md").exists() {
        cwd.join("MEMORY.md")
    } else if cwd.join("ELAI.md").exists() {
        cwd.join("ELAI.md")
    } else {
        cwd.join("MEMORY.md")
    };
    let timestamp = {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    };
    let cumulative = usage.cumulative_usage();
    let summary = format!(
        "\n## Budget Save — {timestamp}\n- Reason: {reason}\n- Tokens: {}/{}\n- Turns: {}\n- Cost: ${:.4}\n- Model: {model}\n",
        cumulative.total_tokens(),
        pct.tokens_pct,
        usage.turns(),
        pct.current_cost_usd,
    );
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&memory_path)?;
    file.write_all(summary.as_bytes())
}

fn handle_tui_slash_command(
    cmd: String,
    app: &mut tui::UiApp,
    session: &Arc<std::sync::Mutex<Session>>,
    budget_tracker: &std::sync::Arc<std::sync::Mutex<BudgetTracker>>,
    msg_tx: &mpsc::Sender<tui::TuiMsg>,
) {
    // Remove the thinking flag since slash commands don't call the runtime.
    app.thinking = false;
    let raw = cmd.trim_start_matches('/');
    // Support `/model <name>` with an argument.
    let (base, arg) = if let Some((b, a)) = raw.split_once(' ') {
        (b, Some(a.trim()))
    } else {
        (raw, None)
    };

    match base {
        "clear" => {
            app.chat.clear();
            app.chat_scroll = 0;
            *session.lock().unwrap() = Session::new();
            // Reativa o overlay de dicas — o reaparecimento é o feedback visual
            // de "histórico limpo" sem precisar empurrar uma SystemNote que
            // contaminaria o `app.chat.is_empty()` checado pelo render_tips.
            app.reset_tips();
        }
        "help" => {
            let help = format!(
                "\
{header}\n\
  /help          {help}\n\
  /status        {status}\n\
  /model [nome]  {model}\n\
  /permissions   {permissions}\n\
  /session [id]  {session}\n\
  /clear         {clear}\n\
  /cost          {cost}\n\
  /compact       {compact}\n\
  /export        {export}\n\
  /memory        {memory}\n\
  /dream         {dream}\n\
  /init          {init}\n\
  /verify        {verify}\n\
  /theme gray <n> {theme_gray}\n\
  /swd [off|partial|full]  {swd}\n\
  /keys          {keys}\n\
  /uninstall     {uninstall}\n\
  /version       {version}\n\
  /locale [pt-BR|en] {locale}\n\
  /exit          {exit}\n\
{shortcuts}",
                header = rust_i18n::t!("tui.repl.help.header"),
                help = rust_i18n::t!("tui.repl.help.help"),
                status = rust_i18n::t!("tui.repl.help.status"),
                model = rust_i18n::t!("tui.repl.help.model"),
                permissions = rust_i18n::t!("tui.repl.help.permissions"),
                session = rust_i18n::t!("tui.repl.help.session"),
                clear = rust_i18n::t!("tui.repl.help.clear"),
                cost = rust_i18n::t!("tui.repl.help.cost"),
                compact = rust_i18n::t!("tui.repl.help.compact"),
                export = rust_i18n::t!("tui.repl.help.export"),
                memory = rust_i18n::t!("tui.repl.help.memory"),
                dream = rust_i18n::t!("tui.repl.help.dream"),
                init = rust_i18n::t!("tui.repl.help.init"),
                verify = rust_i18n::t!("tui.repl.help.verify"),
                theme_gray = rust_i18n::t!("tui.repl.help.theme_gray"),
                swd = rust_i18n::t!("tui.repl.help.swd"),
                keys = rust_i18n::t!("tui.repl.help.keys"),
                uninstall = rust_i18n::t!("tui.repl.help.uninstall"),
                version = rust_i18n::t!("tui.repl.help.version"),
                locale = rust_i18n::t!("tui.repl.help.locale"),
                exit = rust_i18n::t!("tui.repl.help.exit"),
                shortcuts = rust_i18n::t!("tui.repl.help.shortcuts"),
            );
            app.push_chat(tui::ChatEntry::SystemNote(help));
        }
        "status" => {
            let cost = estimate_tui_cost(app);
            let msgs = session.lock().map(|g| g.messages.len()).unwrap_or(0);
            app.push_chat(tui::ChatEntry::SystemNote(format!(
                "{header}\n  {model:<11} {}\n  {permissions:<11} {}\n  {session:<11} {}\n  {messages:<11} {msgs}\n  {tokens:<11} {} / {tokens_out} {}\n  {cost_estimate:<11} ${cost:.4}",
                app.model, app.permission_mode, app.session_id,
                app.input_tokens, app.output_tokens,
                header = rust_i18n::t!("tui.repl.status.header"),
                model = rust_i18n::t!("tui.repl.status.model"),
                permissions = rust_i18n::t!("tui.repl.status.permissions"),
                session = rust_i18n::t!("tui.repl.status.session"),
                messages = rust_i18n::t!("tui.repl.status.messages"),
                tokens = rust_i18n::t!("tui.repl.status.tokens"),
                tokens_out = rust_i18n::t!("tui.repl.status.tokens_out"),
                cost_estimate = rust_i18n::t!("tui.repl.status.cost_estimate"),
            )));
        }
        "model" => {
            if let Some(model_name) = arg {
                let m = model_name.to_string();
                app.model = m.clone();
                app.push_chat(tui::ChatEntry::SystemNote(
                    rust_i18n::t!("tui.repl.feedback.model_changed", model = m).to_string(),
                ));
            } else {
                app.open_model_picker();
            }
        }
        "permissions" => {
            if let Some(perm) = arg {
                app.permission_mode = perm.to_string();
                app.push_chat(tui::ChatEntry::SystemNote(
                    rust_i18n::t!("tui.repl.feedback.permissions_changed", mode = perm.to_string()).to_string(),
                ));
            } else {
                app.open_permission_picker();
            }
        }
        "session" => {
            if let Some(session_id) = arg {
                if let Ok(handle) = resolve_session_reference(session_id) {
                    if let Ok(loaded) = Session::load_from_path(&handle.path) {
                        let msg_count = loaded.messages.len();
                        sync_session_to_app_chat(&loaded, app);
                        *session.lock().unwrap() = loaded;
                        app.push_chat(tui::ChatEntry::SystemNote(
                            rust_i18n::t!(
                                "tui.repl.feedback.session_resumed",
                                id = session_id.to_string(),
                                count = msg_count.to_string()
                            ).to_string(),
                        ));
                    }
                }
            } else {
                app.open_session_picker();
            }
        }
        "cost" => {
            let cost = estimate_tui_cost(app);
            app.push_chat(tui::ChatEntry::SystemNote(
                rust_i18n::t!(
                    "tui.repl.feedback.cost_estimate",
                    cost = format!("{cost:.4}"),
                    tokens_in = app.input_tokens.to_string(),
                    tokens_out = app.output_tokens.to_string()
                ).to_string(),
            ));
        }
        "version" => {
            app.push_chat(tui::ChatEntry::SystemNote(
                rust_i18n::t!("tui.repl.feedback.version_line", version = VERSION).to_string(),
            ));
        }
        "diff" => {
            let diff = Command::new("git")
                .args(["diff", "--stat"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_else(|_| "git diff failed".to_string());
            let out = if diff.trim().is_empty() {
                rust_i18n::t!("tui.repl.feedback.no_git_changes").to_string()
            } else {
                diff
            };
            app.push_chat(tui::ChatEntry::SystemNote(out));
        }
        "compact" => {
            let mut guard = session.lock().unwrap();
            let total = guard.messages.len();
            let keep = 20;
            if total > keep {
                guard.messages.drain(0..total - keep);
                app.push_chat(tui::ChatEntry::SystemNote(
                    rust_i18n::t!(
                        "tui.repl.feedback.compact_done",
                        from = total.to_string(),
                        to = keep.to_string()
                    ).to_string(),
                ));
            } else {
                app.push_chat(tui::ChatEntry::SystemNote(
                    rust_i18n::t!(
                        "tui.repl.feedback.compact_already",
                        count = total.to_string()
                    ).to_string(),
                ));
            }
        }
        "export" => {
            let guard = session.lock().unwrap();
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let filename = format!("elai-export-{ts}.txt");
            let mut content = String::new();
            for msg in &guard.messages {
                let role_str = match msg.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                };
                let mut text = String::new();
                for block in &msg.blocks {
                    if let ContentBlock::Text { text: t } = block {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                }
                let _ = writeln!(content, "[{role_str}]\n{text}\n");
            }
            drop(guard);
            match fs::write(&filename, &content) {
                Ok(_) => app.push_chat(tui::ChatEntry::SystemNote(
                    rust_i18n::t!("tui.repl.feedback.export_ok", file = filename).to_string(),
                )),
                Err(e) => app.push_chat(tui::ChatEntry::SystemNote(
                    rust_i18n::t!("tui.repl.feedback.export_err", error = e.to_string()).to_string(),
                )),
            }
        }
        "memory" => {
            let cwd = env::current_dir().unwrap_or_default();
            let found = ["ELAI.md", "CLAUDE.md", ".elai/memory.md"]
                .iter()
                .map(|f| cwd.join(f))
                .find(|p| p.exists());
            match found {
                Some(path) => match fs::read_to_string(&path) {
                    Ok(content) => {
                        let preview: String =
                            content.lines().take(50).collect::<Vec<_>>().join("\n");
                        app.push_chat(tui::ChatEntry::SystemNote(format!(
                            "📄 {}\n{preview}",
                            path.display()
                        )));
                    }
                    Err(e) => app.push_chat(tui::ChatEntry::SystemNote(format!("❌ {e}"))),
                },
                None => app.push_chat(tui::ChatEntry::SystemNote(
                    rust_i18n::t!("tui.repl.feedback.memory_not_found").to_string(),
                )),
            }
        }
        "init" => {
            // Spawn em thread para não bloquear o TUI durante a indexação.
            // O progresso vai pro chat via `TuiMsg::TaskProgress` (linha viva
            // única) — o sink default global já foi configurado no startup do
            // TUI pra mandar nesse canal. O `report.render()` final volta como
            // `SystemNote` porque é texto multilinha de resumo, não progresso.
            let tx = msg_tx.clone();
            let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let args = crate::args::InitArgs::default();
            std::thread::spawn(move || match initialize_repo(&cwd, &args) {
                Ok(report) => {
                    let _ = tx.send(tui::TuiMsg::SystemNote(report.render()));
                }
                Err(e) => {
                    let _ = tx.send(tui::TuiMsg::Error(format!("init: {e}")));
                }
            });
            app.push_chat(tui::ChatEntry::SystemNote(
                rust_i18n::t!("tui.repl.feedback.init_started").to_string(),
            ));
        }
        "verify" => {
            let tx = msg_tx.clone();
            let cwd = env::current_dir().unwrap_or_default();
            std::thread::spawn(move || {
                let outcome = runtime::with_task_default(
                    runtime::TaskType::LocalWorkflow,
                    format!("elai verify {}", cwd.display()),
                    "Verify",
                    None,
                    |reporter| verify::run_verify_inner(&cwd, reporter),
                );
                match outcome {
                    Ok((report, _)) => {
                        let _ = tx.send(tui::TuiMsg::SystemNote(verify::render_verify_report_tui(&report)));
                    }
                    Err(e) => {
                        let _ = tx.send(tui::TuiMsg::Error(format!("verify: {e}")));
                    }
                }
            });
            app.push_chat(tui::ChatEntry::SystemNote(
                rust_i18n::t!("tui.repl.feedback.verify_started").to_string(),
            ));
        }
        "plugin" | "plugins" => {
            let tx = msg_tx.clone();
            let raw = arg.unwrap_or("").trim().to_string();
            std::thread::spawn(move || {
                let send = |s: &str| { let _ = tx.send(tui::TuiMsg::SystemNote(s.to_string())); };
                let mut parts = raw.splitn(2, char::is_whitespace);
                let action = parts.next().filter(|s| !s.is_empty()).map(str::to_owned);
                let target = parts.next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);

                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let loader = runtime::ConfigLoader::default_for(&cwd);
                let cfg = match loader.load() {
                    Ok(c) => c,
                    Err(e) => { let _ = tx.send(tui::TuiMsg::Error(format!("plugin: {e}"))); return; }
                };
                let mut manager = build_plugin_manager(&cwd, &loader, &cfg);

                match handle_plugins_slash_command(
                    action.as_deref(),
                    target.as_deref(),
                    &mut manager,
                    &send,
                ) {
                    Ok(result) => { let _ = tx.send(tui::TuiMsg::SystemNote(result.message)); }
                    Err(e) => { let _ = tx.send(tui::TuiMsg::Error(format!("plugin: {e}"))); }
                }
            });
            app.push_chat(tui::ChatEntry::SystemNote(
                rust_i18n::t!("tui.repl.feedback.plugin_started").to_string(),
            ));
        }
        "swd" => {
            use std::sync::atomic::Ordering;
            use crate::swd::SwdLevel;
            let current = SwdLevel::from_u8(app.swd_level.load(Ordering::Relaxed));
            let new_level = if let Some(level_str) = arg {
                match SwdLevel::from_str(level_str) {
                    Some(l) => l,
                    None => {
                        app.push_chat(tui::ChatEntry::SystemNote(
                            rust_i18n::t!(
                                "tui.repl.feedback.swd_invalid",
                                level = level_str.to_string()
                            ).to_string(),
                        ));
                        return;
                    }
                }
            } else {
                current.cycle()
            };
            app.swd_level.store(new_level as u8, Ordering::Relaxed);
            app.push_chat(tui::ChatEntry::SystemNote(
                rust_i18n::t!(
                    "tui.repl.feedback.swd_changed",
                    from = current.as_str().to_string(),
                    to = new_level.as_str().to_string()
                ).to_string(),
            ));
        }
        "keys" | "setup" => {
            app.open_auth_picker();
        }
        "uninstall" => {
            app.open_uninstall_confirm();
        }
        "agents" => {
            let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let output = handle_agents_slash_command(arg, &cwd)
                .unwrap_or_else(|e| format!("agents: {e}"));
            app.push_chat(tui::ChatEntry::SystemNote(output));
        }
        "skills" => {
            let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let output = handle_skills_slash_command(arg, &cwd)
                .unwrap_or_else(|e| format!("skills: {e}"));
            app.push_chat(tui::ChatEntry::SystemNote(output));
        }
        "budget" => {
            if let Some(a) = arg {
                if a == "off" {
                    budget_tracker.lock().unwrap().disable();
                    app.budget_enabled = false;
                    app.push_chat(tui::ChatEntry::SystemNote(
                        rust_i18n::t!("tui.repl.feedback.budget_off").to_string(),
                    ));
                } else {
                    let parts: Vec<&str> = a.split_whitespace().collect();
                    let max_tokens = parts.first().and_then(|s| s.parse::<u64>().ok());
                    let max_usd = parts.get(1).and_then(|s| s.parse::<f64>().ok());
                    if max_tokens.is_some() || max_usd.is_some() {
                        let cfg = BudgetConfig {
                            max_tokens,
                            max_turns: None,
                            max_cost_usd: max_usd,
                            warn_at_pct: 80.0,
                        };
                        let cwd = std::env::current_dir().unwrap_or_default();
                        let _ = save_budget_config(&cwd, &cfg);
                        budget_tracker.lock().unwrap().update_config(cfg.clone());
                        app.budget_enabled = true;
                        app.push_chat(tui::ChatEntry::SystemNote(
                            rust_i18n::t!(
                                "tui.repl.feedback.budget_set",
                                tokens = cfg.max_tokens.map_or("∞".into(), |t| t.to_string()),
                                usd = cfg.max_cost_usd.map_or("∞".into(), |u| format!("${u:.2}"))
                            ).to_string(),
                        ));
                    } else {
                        app.push_chat(tui::ChatEntry::SystemNote(
                            rust_i18n::t!("tui.repl.feedback.budget_usage_hint").to_string(),
                        ));
                    }
                }
            } else {
                let bt = budget_tracker.lock().unwrap();
                let usage = {
                    let guard = session.lock().unwrap();
                    UsageTracker::from_session(&guard)
                };
                if bt.is_enabled() {
                    let pct = bt.usage_pct(&usage, &app.model);
                    let cfg = bt.config();
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "📊 Budget: {:.0}% consumido · ${:.4} · Tokens: {} · Turns: {}\n   Limites: tokens={} usd={} turns={}",
                        pct.highest_pct,
                        pct.current_cost_usd,
                        pct.total_tokens,
                        usage.turns(),
                        cfg.max_tokens.map_or("∞".into(), |t| t.to_string()),
                        cfg.max_cost_usd.map_or("∞".into(), |u| format!("${u:.2}")),
                        cfg.max_turns.map_or("∞".into(), |t| t.to_string()),
                    )));
                } else {
                    app.push_chat(tui::ChatEntry::SystemNote(
                        "ℹ️  Budget desativado. Use /budget <tokens> [usd] para ativar"
                            .to_string(),
                    ));
                }
            }
        }
        "stats" => {
            use commands::stats::render_stats_report;
            use runtime::{default_telemetry_path, load_entries};
            use std::time::{SystemTime, UNIX_EPOCH};

            let days: Option<u32> = arg.and_then(|a| {
                let mut toks = a.split_whitespace();
                while let Some(tok) = toks.next() {
                    if tok == "--days" {
                        return toks.next()?.parse().ok();
                    } else if let Some(s) = tok.strip_prefix("--days=") {
                        return s.parse().ok();
                    }
                }
                None
            });
            let by_model = arg.map_or(true, |a| a.contains("--by-model") || !a.contains("--by-project"));
            let by_project = arg.map_or(false, |a| a.contains("--by-project"));
            let path = default_telemetry_path();
            let since_secs: Option<u64> = days.map(|d| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .saturating_sub(u64::from(d) * 86400)
            });
            let output = match load_entries(&path, since_secs) {
                Ok(entries) => render_stats_report(&entries, by_model, by_project, days),
                Err(e) => format!("Error reading telemetry: {e}"),
            };
            app.push_chat(tui::ChatEntry::SystemNote(output));
        }
        "providers" => {
            use commands::providers::render_providers_dashboard;
            use runtime::{default_telemetry_path, load_entries};

            let verbose = arg.map_or(false, |a| a.contains("--verbose"));
            let path = default_telemetry_path();
            let output = match load_entries(&path, None) {
                Ok(entries) => render_providers_dashboard(&entries, verbose),
                Err(e) => format!("Error reading telemetry: {e}"),
            };
            app.push_chat(tui::ChatEntry::SystemNote(output));
        }
        "dream" => {
            let cwd = env::current_dir().unwrap_or_default();
            match dream::find_memory_file(&cwd) {
                None => {
                    app.push_chat(tui::ChatEntry::SystemNote(
                        "Nenhum arquivo de memória encontrado (ELAI.md, CLAUDE.md, .elai/ELAI.md)."
                            .into(),
                    ));
                }
                Some(path) => match fs::read_to_string(&path) {
                    Err(e) => {
                        app.push_chat(tui::ChatEntry::SystemNote(format!(
                            "Erro ao ler arquivo: {e}"
                        )));
                    }
                    Ok(content) => {
                        let parsed = dream::parse_memory_sections(&content);
                        let force = arg.map_or(false, |a| a == "--force");
                        if parsed.old_entries.is_empty() && !force {
                            app.push_chat(tui::ChatEntry::SystemNote(format!(
                                "Dream: nada a comprimir ({} entradas <= 20). Use /dream --force para forçar.",
                                parsed.recent_entries.len()
                            )));
                        } else {
                            let n = if parsed.old_entries.is_empty() {
                                parsed.recent_entries.len().saturating_sub(20)
                            } else {
                                parsed.old_entries.len()
                            };
                            app.push_chat(tui::ChatEntry::SystemNote(format!(
                                "Dream: {} entradas prontas para comprimir em {}. Use /dream no modo REPL para executar com IA.",
                                n,
                                path.display()
                            )));
                        }
                    }
                },
            }
        }
        "theme" => {
            let Some(raw_arg) = arg else {
                app.push_chat(tui::ChatEntry::SystemNote(
                    "Uso: /theme gray <232-255>".to_string(),
                ));
                return;
            };
            let parts: Vec<&str> = raw_arg.split_whitespace().collect();
            if parts.len() != 2 || parts[0] != "gray" {
                app.push_chat(tui::ChatEntry::SystemNote(
                    "Uso: /theme gray <232-255>".to_string(),
                ));
                return;
            }
            let Ok(intensity) = parts[1].parse::<u8>() else {
                app.push_chat(tui::ChatEntry::SystemNote(
                    "❌ Intensidade inválida. Use um número entre 232 e 255.".to_string(),
                ));
                return;
            };
            if !(232..=255).contains(&intensity) {
                app.push_chat(tui::ChatEntry::SystemNote(
                    "❌ Intensidade fora da faixa válida (232..=255).".to_string(),
                ));
                return;
            }
            let mut cfg = runtime::load_global_config().unwrap_or_default();
            cfg.theme.text_secondary_intensity = Some(intensity);
            cfg.theme.text_secondary = None;
            match runtime::save_global_config(&cfg) {
                Ok(()) => {
                    tui::refresh_theme_cache();
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "✅ Tema atualizado: text_secondary = ANSI {intensity}."
                    )));
                }
                Err(e) => {
                    app.push_chat(tui::ChatEntry::SystemNote(format!(
                        "❌ Falha ao salvar ~/.elai/config.json: {e}"
                    )));
                }
            }
        }
        "update" => {
            let tx = msg_tx.clone();
            std::thread::spawn(move || {
                let msg = match crate::updater::check_available() {
                    Some(info) => format!(
                        "🆙 Nova versão disponível: v{} (atual: v{}).\n   Para instalar, execute `elai update` no shell — `apply` está bloqueado dentro do TUI por segurança.",
                        info.latest, info.current
                    ),
                    None => format!(
                        "✓ Você já está na versão mais recente (v{}).",
                        env!("CARGO_PKG_VERSION")
                    ),
                };
                let _ = tx.send(tui::TuiMsg::SystemNote(msg));
            });
            app.push_chat(tui::ChatEntry::SystemNote(
                "Verificando atualizações em background...".to_string(),
            ));
        }
        "config" => {
            let section = arg.map(str::to_owned);
            let tx = msg_tx.clone();
            std::thread::spawn(move || match render_config_report(section.as_deref()) {
                Ok(out) => {
                    let _ = tx.send(tui::TuiMsg::SystemNote(out));
                }
                Err(e) => {
                    let _ = tx.send(tui::TuiMsg::Error(format!("config: {e}")));
                }
            });
        }
        "tools" => {
            let out = handle_tools_slash_command(arg);
            app.push_chat(tui::ChatEntry::SystemNote(out));
        }
        "cache" => {
            let action = arg.map_or("stats", str::trim);
            let cache_path = dirs_home()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".elai")
                .join("cache.json");
            let mut cache = ResponseCache::new(cache_path, ResponseCache::DEFAULT_TTL_MS);
            let note = if action == "clear" {
                cache.clear();
                match cache.flush() {
                    Ok(()) => "✅ Cache limpo.".to_string(),
                    Err(e) => format!("❌ Falha ao gravar cache: {e}"),
                }
            } else {
                let s = cache.stats();
                let oldest_age = s.oldest_entry_ms.map(|ms| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                    let age_secs = now.saturating_sub(ms) / 1000;
                    format!("{age_secs}s atrás")
                });
                format!(
                    "Cache\n  Entradas         {}\n  Total hits       {}\n  Mais antiga      {}",
                    s.total_entries,
                    s.total_hits,
                    oldest_age.as_deref().unwrap_or("—"),
                )
            };
            app.push_chat(tui::ChatEntry::SystemNote(note));
        }
        "debug-tool-call" => {
            let session_arc = session.clone();
            let tx = msg_tx.clone();
            std::thread::spawn(move || {
                let session = session_arc.lock().unwrap().clone();
                match render_last_tool_debug_report(&session) {
                    Ok(out) => {
                        let _ = tx.send(tui::TuiMsg::SystemNote(out));
                    }
                    Err(e) => {
                        let _ = tx.send(tui::TuiMsg::Error(format!("debug-tool-call: {e}")));
                    }
                }
            });
        }
        "resume" => {
            // No modo TUI, /resume reusa o picker de sessões.
            app.open_session_picker();
        }
        "branch" => {
            let tx = msg_tx.clone();
            let cwd = env::current_dir().unwrap_or_default();
            std::thread::spawn(move || {
                let out = std::process::Command::new("git")
                    .args(["branch", "-a"])
                    .current_dir(&cwd)
                    .output();
                let note = match out {
                    Ok(o) if o.status.success() => format!(
                        "🌿 Branches:\n{}",
                        String::from_utf8_lossy(&o.stdout)
                    ),
                    Ok(o) => format!(
                        "git branch falhou:\n{}",
                        String::from_utf8_lossy(&o.stderr)
                    ),
                    Err(e) => format!("Erro ao executar git: {e}"),
                };
                let _ = tx.send(tui::TuiMsg::SystemNote(note));
            });
        }
        "worktree" => {
            let tx = msg_tx.clone();
            let cwd = env::current_dir().unwrap_or_default();
            std::thread::spawn(move || {
                let out = std::process::Command::new("git")
                    .args(["worktree", "list"])
                    .current_dir(&cwd)
                    .output();
                let note = match out {
                    Ok(o) if o.status.success() => format!(
                        "🌳 Worktrees:\n{}",
                        String::from_utf8_lossy(&o.stdout)
                    ),
                    Ok(o) => format!(
                        "git worktree falhou:\n{}",
                        String::from_utf8_lossy(&o.stderr)
                    ),
                    Err(e) => format!("Erro ao executar git: {e}"),
                };
                let _ = tx.send(tui::TuiMsg::SystemNote(note));
            });
        }
        // Comandos AI-driven que ainda não foram migrados para o loop de turnos
        // do TUI. Disponíveis via `elai prompt /<cmd>` no shell por enquanto.
        "bughunter" | "ultraplan" | "teleport" | "commit" | "commit-push-pr"
        | "pr" | "issue" => {
            app.push_chat(tui::ChatEntry::SystemNote(format!(
                "🚧 /{base} em breve no modo TUI — por enquanto execute `elai prompt /{base}` no shell."
            )));
        }
        "locale" => {
            if let Some(lang) = arg {
                let message = handle_locale_command(Some(lang));
                app.push_chat(tui::ChatEntry::SystemNote(message));
            } else {
                let locales: Vec<String> =
                    SUPPORTED_LOCALES.iter().map(|s| s.to_string()).collect();
                let current = rust_i18n::locale().to_string();
                app.open_locale_picker(locales, &current);
            }
        }
        "exit" | "quit" => {
            app.should_quit = true;
        }
        other => {
            app.push_chat(tui::ChatEntry::SystemNote(
                rust_i18n::t!(
                    "tui.repl.feedback.unknown_command",
                    cmd = other.to_string()
                ).to_string(),
            ));
        }
    }
}

#[derive(Debug, Clone)]
struct SessionHandle {
    id: String,
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct ManagedSessionSummary {
    id: String,
    path: PathBuf,
    modified_epoch_secs: u64,
    message_count: usize,
}

struct LiveCli {
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    system_prompt: Vec<String>,
    runtime: ConversationRuntime<DefaultRuntimeClient, CliToolExecutor>,
    session: SessionHandle,
    telemetry: TelemetryHandle,
    _telemetry_shutdown: Option<TelemetryShutdown>,
    session_start: Instant,
    cache: ResponseCache,
    user_commands: UserCommandRegistry,
}

impl LiveCli {
    fn new(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
        no_cache: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let system_prompt = build_system_prompt()?;
        let session = create_managed_session_handle()?;

        // Start telemetry unless disabled.
        let (telemetry, telemetry_shutdown) = start_telemetry();

        let runtime = build_runtime(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            true,
            allowed_tools.clone(),
            permission_mode,
            None,
            telemetry.clone(),
        )?;

        // Initialize response cache.
        let cache = if no_cache {
            ResponseCache::disabled()
        } else {
            let cache_path = dirs_home()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".elai")
                .join("cache.json");
            ResponseCache::new(cache_path, ResponseCache::DEFAULT_TTL_MS)
        };

        let user_commands = {
            let cwd = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            UserCommandRegistry::discover(&cwd).unwrap_or_default()
        };

        let cli = Self {
            model,
            allowed_tools,
            permission_mode,
            system_prompt,
            runtime,
            session,
            telemetry,
            _telemetry_shutdown: telemetry_shutdown,
            session_start: Instant::now(),
            cache,
            user_commands,
        };
        cli.persist_session()?;
        Ok(cli)
    }

    /// Variante de [`Self::new`] que aplica um override de extended thinking e
    /// (opcionalmente) anexa uma seção extra ao system prompt antes de construir
    /// o runtime. Usada pelo CLI headless para suportar `--thinking`/`--ultrathink`
    /// e injetar o `CAPYBARA_SYSTEM_PROMPT` quando thinking está ativo.
    #[allow(clippy::too_many_arguments)]
    fn new_with_thinking(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
        no_cache: bool,
        thinking: Option<ThinkingConfig>,
        extra_system: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut system_prompt = build_system_prompt()?;
        if let Some(extra) = extra_system {
            system_prompt.push(extra);
        }
        let session = create_managed_session_handle()?;

        let (telemetry, telemetry_shutdown) = start_telemetry();

        let runtime = build_runtime_with_thinking(
            Session::new(),
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            true,
            allowed_tools.clone(),
            permission_mode,
            None,
            telemetry.clone(),
            thinking,
        )?;

        let cache = if no_cache {
            ResponseCache::disabled()
        } else {
            let cache_path = dirs_home()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".elai")
                .join("cache.json");
            ResponseCache::new(cache_path, ResponseCache::DEFAULT_TTL_MS)
        };

        let user_commands = {
            let cwd = std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            UserCommandRegistry::discover(&cwd).unwrap_or_default()
        };

        let cli = Self {
            model,
            allowed_tools,
            permission_mode,
            system_prompt,
            runtime,
            session,
            telemetry,
            _telemetry_shutdown: telemetry_shutdown,
            session_start: Instant::now(),
            cache,
            user_commands,
        };
        cli.persist_session()?;
        Ok(cli)
    }

    fn startup_banner(&self) -> String {
        let cwd = env::current_dir().map_or_else(
            |_| "<unknown>".to_string(),
            |path| path.display().to_string(),
        );
        format!(
            "\n\
\x1b[38;5;215m╭──────────────────────────────────────────────────────────────────────────────╮\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                                                              \x1b[38;5;215m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                \x1b[38;5;216m███████╗██╗      █████╗ ██╗\x1b[0m             \x1b[38;5;202m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                \x1b[38;5;216m██╔════╝██║     ██╔══██╗██║\x1b[0m             \x1b[38;5;202m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                \x1b[38;5;216m█████╗  ██║     ███████║██║\x1b[0m             \x1b[38;5;202m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                \x1b[38;5;216m██╔══╝  ██║     ██╔══██║██║\x1b[0m             \x1b[38;5;202m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                \x1b[38;5;216m███████╗███████╗██║  ██║██║\x1b[0m             \x1b[38;5;202m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                \x1b[38;5;216m╚══════╝╚══════╝╚═╝  ╚═╝╚═╝\x1b[0m             \x1b[38;5;202m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                                                              \x1b[38;5;215m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m   \x1b[2mModel\x1b[0m            {} \x1b[38;5;215m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m   \x1b[2mPermissions\x1b[0m      {} \x1b[38;5;215m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m   \x1b[2mDirectory\x1b[0m        {} \x1b[38;5;215m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m   \x1b[2mSession\x1b[0m          {} \x1b[38;5;215m│\x1b[0m\n\
\x1b[38;5;215m│\x1b[0m                                                                              \x1b[38;5;215m│\x1b[0m\n\
\x1b[38;5;215m╰──────────────────────────────────────────────────────────────────────────────╯\x1b[0m\n\
\n\
Type \x1b[1m/help\x1b[0m for commands · \x1b[2mShift+Enter\x1b[0m for newline",
            self.model,
            self.permission_mode.as_str(),
            cwd,
            self.session.id,
        )
    }

    fn run_turn(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Enriquece o input com conteúdo de arquivos mencionados via `@<path>`.
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let enriched = enrich_input_with_mentions(input, &cwd);
        let input = enriched.as_str();

        // Build cache key from current session messages + this user input.
        let cache_key = {
            let mut msgs = self.runtime.session().messages.clone();
            msgs.push(ConversationMessage::user_text(input));
            generate_cache_key(&msgs, &self.model, &self.system_prompt)
        };

        // Cache hit: serve from cache without calling the API.
        if let Some(ref key) = cache_key {
            if let Some(cached) = self.cache.get(key) {
                println!("{}", cached.response_json);
                return Ok(());
            }
        }

        let mut spinner = Spinner::new();
        let mut stdout = io::stdout();
        spinner.tick(
            "🦀 Thinking...",
            TerminalRenderer::new().color_theme(),
            &mut stdout,
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let result = self.runtime.run_turn(input, Some(&mut permission_prompter));
        match result {
            Ok(summary) => {
                // Store the assistant text in cache (only for tool-free turns).
                if let Some(key) = cache_key {
                    let response_text = summary
                        .assistant_messages
                        .iter()
                        .filter_map(|m| {
                            m.blocks.iter().find_map(|b| {
                                if let runtime::ContentBlock::Text { text } = b {
                                    Some(text.clone())
                                } else {
                                    None
                                }
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !response_text.is_empty() {
                        self.cache.put(key, CachedResponse {
                            response_json: response_text,
                            model: self.model.clone(),
                            created_at_ms: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0),
                            hit_count: 0,
                        });
                    }
                }
                // Spinner finish clears the *current* terminal line. Streamed assistant text often
                // leaves the cursor on the last line of output without a trailing newline, so we must
                // move to a fresh line first or the clear wipes the visible reply.
                writeln!(stdout)?;
                spinner.finish(
                    "✨ Done",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                println!();
                self.emit_turn_telemetry(&summary, None);
                self.persist_session()?;

                // Auto-dream evaluation (post-turn). Avalia gates e, se Fire, libera lock
                // imediatamente (execução completa do agent forked é deferida para versão futura).
                let cwd = std::env::current_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                let cfg = runtime::AutoDreamConfig::from_env();
                match runtime::evaluate_auto_dream(&cwd, &cfg) {
                    runtime::AutoDreamDecision::Fire {
                        session_ids,
                        hours_since_last,
                        prior_mtime_ms,
                    } => {
                        eprintln!(
                            "[auto-dream] gates open: {} sessions, {:.1}h since last consolidation. \
                             Running consolidation deferred to v0.8.0; releasing lock.",
                            session_ids.len(),
                            hours_since_last,
                        );
                        let _ = runtime::auto_dream::rollback_lock(&cwd, prior_mtime_ms);
                    }
                    runtime::AutoDreamDecision::Skip { reason } => {
                        if std::env::var_os("ELAI_AUTO_DREAM_DEBUG").is_some() {
                            eprintln!("[auto-dream] skipped: {reason:?}");
                        }
                    }
                }

                Ok(())
            }
            Err(error) => {
                writeln!(stdout)?;
                spinner.fail(
                    "❌ Request failed",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                Err(Box::new(error))
            }
        }
    }

    fn emit_turn_telemetry(&self, summary: &runtime::TurnSummary, error_type: Option<&str>) {
        use runtime::{default_telemetry_path, now_iso8601, pricing_for_model, TelemetryEntry, TelemetryWriter};
        let usage = &summary.usage;
        let cost_usd = pricing_for_model(&self.model)
            .map(|p| {
                f64::from(usage.input_tokens) * p.input_cost_per_million / 1_000_000.0
                    + f64::from(usage.output_tokens) * p.output_cost_per_million / 1_000_000.0
                    + f64::from(usage.cache_creation_input_tokens)
                        * p.cache_creation_cost_per_million
                        / 1_000_000.0
                    + f64::from(usage.cache_read_input_tokens)
                        * p.cache_read_cost_per_million
                        / 1_000_000.0
            })
            .unwrap_or(0.0);
        let project = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "unknown".to_string());
        let entry = TelemetryEntry {
            timestamp: now_iso8601(),
            session_id: self.session.id.clone(),
            project,
            model: self.model.clone(),
            input_tokens: u64::from(usage.input_tokens),
            output_tokens: u64::from(usage.output_tokens),
            cache_write_tokens: u64::from(usage.cache_creation_input_tokens),
            cache_read_tokens: u64::from(usage.cache_read_input_tokens),
            cost_usd,
            latency_ms: self.session_start.elapsed().as_millis() as u64,
            success: error_type.is_none(),
            provider: None,
            error_type: error_type.map(str::to_string),
        };
        let writer = TelemetryWriter::new(default_telemetry_path());
        let _ = writer.append(&entry);
        self.telemetry.emit(TelemetryEvent::TokenUsage {
            timestamp_ms: now_millis(),
            model: self.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            cache_write_tokens: usage.cache_creation_input_tokens,
            cost_usd,
        });
    }

    fn run_turn_with_output(
        &mut self,
        input: &str,
        output_format: CliOutputFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match output_format {
            CliOutputFormat::Text => self.run_turn(input),
            CliOutputFormat::Json => self.run_prompt_json(input),
        }
    }

    fn run_prompt_json(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        let session = self.runtime.session().clone();
        let mut runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            false,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
            self.telemetry.clone(),
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let summary = runtime.run_turn(input, Some(&mut permission_prompter))?;
        self.runtime = runtime;
        self.persist_session()?;
        println!(
            "{}",
            json!({
                "message": final_assistant_text(&summary),
                "model": self.model,
                "iterations": summary.iterations,
                "tool_uses": collect_tool_uses(&summary),
                "tool_results": collect_tool_results(&summary),
                "usage": {
                    "input_tokens": summary.usage.input_tokens,
                    "output_tokens": summary.usage.output_tokens,
                    "cache_creation_input_tokens": summary.usage.cache_creation_input_tokens,
                    "cache_read_input_tokens": summary.usage.cache_read_input_tokens,
                }
            })
        );
        Ok(())
    }

    fn repl_feature_not_wired(message: &str) -> bool {
        eprintln!("{message}");
        false
    }

    fn handle_repl_command(
        &mut self,
        command: SlashCommand,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(match command {
            SlashCommand::Help => {
                println!("{}", render_repl_help());
                false
            }
            SlashCommand::Status => {
                self.print_status();
                false
            }
            SlashCommand::Bughunter { scope } => {
                self.run_bughunter(scope.as_deref())?;
                false
            }
            SlashCommand::Commit => {
                self.run_commit()?;
                true
            }
            SlashCommand::Pr { context } => {
                self.run_pr(context.as_deref())?;
                false
            }
            SlashCommand::Issue { context } => {
                self.run_issue(context.as_deref())?;
                false
            }
            SlashCommand::Ultraplan { task } => {
                self.run_ultraplan(task.as_deref())?;
                false
            }
            SlashCommand::Teleport { target } => {
                self.run_teleport(target.as_deref())?;
                false
            }
            SlashCommand::DebugToolCall => {
                self.run_debug_tool_call()?;
                false
            }
            SlashCommand::Compact => {
                self.compact()?;
                false
            }
            SlashCommand::Model { model } => self.set_model(model)?,
            SlashCommand::Permissions { mode } => self.set_permissions(mode)?,
            SlashCommand::Clear { confirm } => self.clear_session(confirm)?,
            SlashCommand::Cost => {
                self.print_cost();
                false
            }
            SlashCommand::Resume { session_path } => self.resume_session(session_path)?,
            SlashCommand::Config { section } => {
                Self::print_config(section.as_deref())?;
                false
            }
            SlashCommand::Memory => {
                Self::print_memory()?;
                false
            }
            SlashCommand::Init => {
                // /init na TUI usa defaults; pra customizar use `elai init --flags` no shell.
                run_init(&crate::args::InitArgs::default())?;
                false
            }
            SlashCommand::Diff => {
                Self::print_diff()?;
                false
            }
            SlashCommand::Version => {
                Self::print_version();
                false
            }
            SlashCommand::Update => {
                updater::run_update();
                false
            }
            SlashCommand::Export { path } => {
                self.export_session(path.as_deref())?;
                false
            }
            SlashCommand::Session { action, target } => {
                self.handle_session_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Plugins { action, target } => {
                self.handle_plugins_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Agents { args } => {
                Self::print_agents(args.as_deref())?;
                false
            }
            SlashCommand::Skills { args } => {
                Self::print_skills(args.as_deref())?;
                false
            }
            SlashCommand::Branch { .. } => Self::repl_feature_not_wired(
                "git branch commands not yet wired to REPL",
            ),
            SlashCommand::Worktree { .. } => Self::repl_feature_not_wired(
                "git worktree commands not yet wired to REPL",
            ),
            SlashCommand::CommitPushPr { context } => {
                let cwd = std::env::current_dir()?;

                let staged_stat = std::process::Command::new("git")
                    .args(["diff", "--cached", "--stat"])
                    .current_dir(&cwd)
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .unwrap_or_default();
                let unstaged_stat = std::process::Command::new("git")
                    .args(["diff", "--stat"])
                    .current_dir(&cwd)
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .unwrap_or_default();

                let context_str = context.as_deref().unwrap_or("");
                let prompt = format!(
                    "Generate a concise commit message, PR title, and PR body for the following changes.\n\
                     Context: {}\n\nStaged:\n{}\n\nUnstaged:\n{}\n\n\
                     Respond as JSON: {{\"commit_message\": \"...\", \"pr_title\": \"...\", \"pr_body\": \"...\"}}",
                    context_str, staged_stat, unstaged_stat
                );

                let response = self.run_internal_prompt_text(&prompt, false)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                let parsed: serde_json::Value = serde_json::from_str(&response)
                    .map_err(|e| std::io::Error::other(format!("AI returned invalid JSON: {e}\n--- response ---\n{response}")))?;

                let request = CommitPushPrRequest {
                    commit_message: parsed["commit_message"].as_str().map(|s| s.to_string()),
                    pr_title: parsed["pr_title"].as_str().unwrap_or("Update").to_string(),
                    pr_body: parsed["pr_body"].as_str().unwrap_or("").to_string(),
                    branch_name_hint: String::new(),
                };

                let outcome = runtime::with_task_default(
                    runtime::TaskType::LocalWorkflow,
                    "elai commit-push-pr",
                    "Commit/Push/PR",
                    None,
                    |reporter| handle_commit_push_pr_slash_command(&request, &cwd, reporter),
                );
                match outcome {
                    Ok(report) => println!("{report}"),
                    Err(e) => eprintln!("commit-push-pr: {e}"),
                }
                false
            }
            SlashCommand::Budget { .. } => {
                Self::repl_feature_not_wired("budget command available in TUI mode")
            }
            SlashCommand::Tools { subcommand } => {
                println!("{}", handle_tools_slash_command(subcommand.as_deref()));
                false
            }
            SlashCommand::Dream { force } => {
                self.run_dream(force)?;
                false
            }
            SlashCommand::Stats { days } => {
                use commands::stats::render_stats_report;
                use runtime::{default_telemetry_path, load_entries};
                use std::time::{SystemTime, UNIX_EPOCH};
                let path = default_telemetry_path();
                let since_secs: Option<u64> = days.map(|d| {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        .saturating_sub(u64::from(d) * 86400)
                });
                match load_entries(&path, since_secs) {
                    Ok(entries) => print!("{}", render_stats_report(&entries, true, false, days)),
                    Err(e) => eprintln!("error reading telemetry: {e}"),
                }
                false
            }
            SlashCommand::Providers { verbose } => {
                use commands::providers::render_providers_dashboard;
                use runtime::{default_telemetry_path, load_entries};
                let path = default_telemetry_path();
                match load_entries(&path, None) {
                    Ok(entries) => print!("{}", render_providers_dashboard(&entries, verbose)),
                    Err(e) => eprintln!("error reading telemetry: {e}"),
                }
                false
            }
            SlashCommand::Cache { subcommand } => {
                self.handle_cache_command(subcommand.as_deref());
                false
            }
            SlashCommand::Verify => {
                match verify::run_verify(&std::env::current_dir().unwrap_or_default()) {
                    Ok(output) => { println!("{output}"); }
                    Err(e) => eprintln!("error running verify: {e}"),
                }
                false
            }
            SlashCommand::Locale { lang } => {
                println!("{}", handle_locale_command(lang.as_deref()));
                false
            }
            SlashCommand::Unknown(name) => {
                Self::repl_feature_not_wired(&format!("unknown slash command: /{name}"))
            }
        })
    }

    fn persist_session(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime.session().save_to_path(&self.session.path)?;
        Ok(())
    }

    fn handle_cache_command(&mut self, subcommand: Option<&str>) {
        match subcommand.map(str::trim).unwrap_or("stats") {
            "clear" => {
                self.cache.clear();
                let _ = self.cache.flush();
                println!("Cache cleared.");
            }
            _ => {
                let s = self.cache.stats();
                let oldest_age = s.oldest_entry_ms.map(|ms| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let age_secs = now.saturating_sub(ms) / 1000;
                    format!("{age_secs}s ago")
                });
                println!(
                    "Cache
  Entries          {}
  Total hits       {}
  Oldest entry     {}",
                    s.total_entries,
                    s.total_hits,
                    oldest_age.as_deref().unwrap_or("—"),
                );
            }
        }
    }

    fn print_status(&self) {
        let cumulative = self.runtime.usage().cumulative_usage();
        let latest = self.runtime.usage().current_turn_usage();
        println!(
            "{}",
            format_status_report(
                &self.model,
                StatusUsage {
                    message_count: self.runtime.session().messages.len(),
                    turns: self.runtime.usage().turns(),
                    latest,
                    cumulative,
                    estimated_tokens: self.runtime.estimated_tokens(),
                },
                self.permission_mode.as_str(),
                &status_context(Some(&self.session.path)).expect("status context should load"),
            )
        );
    }

    fn set_model(&mut self, model: Option<String>) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(model) = model else {
            println!(
                "{}",
                format_model_report(
                    &self.model,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                )
            );
            return Ok(false);
        };

        let model = resolve_model_alias(&model);

        if model == self.model {
            println!(
                "{}",
                format_model_report(
                    &self.model,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                )
            );
            return Ok(false);
        }

        let previous = self.model.clone();
        let session = self.runtime.session().clone();
        let message_count = session.messages.len();
        self.runtime = build_runtime(
            session,
            model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
            self.telemetry.clone(),
        )?;
        self.model.clone_from(&model);
        println!(
            "{}",
            format_model_switch_report(&previous, &model, message_count)
        );
        Ok(true)
    }

    fn set_permissions(
        &mut self,
        mode: Option<String>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(mode) = mode else {
            println!(
                "{}",
                format_permissions_report(self.permission_mode.as_str())
            );
            return Ok(false);
        };

        let normalized = normalize_permission_mode(&mode).ok_or_else(|| {
            format!(
                "unsupported permission mode '{mode}'. Use read-only, workspace-write, or danger-full-access."
            )
        })?;

        if normalized == self.permission_mode.as_str() {
            println!("{}", format_permissions_report(normalized));
            return Ok(false);
        }

        let previous = self.permission_mode.as_str().to_string();
        let session = self.runtime.session().clone();
        self.permission_mode = permission_mode_from_label(normalized);
        self.runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
            self.telemetry.clone(),
        )?;
        println!(
            "{}",
            format_permissions_switch_report(&previous, normalized)
        );
        Ok(true)
    }

    fn clear_session(&mut self, confirm: bool) -> Result<bool, Box<dyn std::error::Error>> {
        if !confirm {
            println!(
                "clear: confirmation required; run /clear --confirm to start a fresh session."
            );
            return Ok(false);
        }

        self.session = create_managed_session_handle()?;
        self.runtime = build_runtime(
            Session::new(),
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
            self.telemetry.clone(),
        )?;
        println!(
            "Session cleared\n  Mode             fresh session\n  Preserved model  {}\n  Permission mode  {}\n  Session          {}",
            self.model,
            self.permission_mode.as_str(),
            self.session.id,
        );
        Ok(true)
    }

    fn print_cost(&self) {
        let cumulative = self.runtime.usage().cumulative_usage();
        println!("{}", format_cost_report(cumulative));
    }

    fn resume_session(
        &mut self,
        session_path: Option<String>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(session_ref) = session_path else {
            println!("Usage: /resume <session-path>");
            return Ok(false);
        };

        let handle = resolve_session_reference(&session_ref)?;
        let session = Session::load_from_path(&handle.path)?;
        let message_count = session.messages.len();
        self.runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
            self.telemetry.clone(),
        )?;
        self.session = handle;
        println!(
            "{}",
            format_resume_report(
                &self.session.path.display().to_string(),
                message_count,
                self.runtime.usage().turns(),
            )
        );
        Ok(true)
    }

    fn print_config(section: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_config_report(section)?);
        Ok(())
    }

    fn print_memory() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_memory_report()?);
        Ok(())
    }

    fn print_agents(args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        println!("{}", handle_agents_slash_command(args, &cwd)?);
        Ok(())
    }

    fn print_skills(args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        println!("{}", handle_skills_slash_command(args, &cwd)?);
        Ok(())
    }

    fn print_diff() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_diff_report()?);
        Ok(())
    }

    fn print_version() {
        println!("{}", render_version_report());
    }

    fn export_session(
        &self,
        requested_path: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let export_path = resolve_export_path(requested_path, self.runtime.session())?;
        fs::write(&export_path, render_export_text(self.runtime.session()))?;
        println!(
            "Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}",
            export_path.display(),
            self.runtime.session().messages.len(),
        );
        Ok(())
    }

    fn handle_session_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        match action {
            None | Some("list") => {
                println!("{}", render_session_list(&self.session.id)?);
                Ok(false)
            }
            Some("switch") => {
                let Some(target) = target else {
                    println!("Usage: /session switch <session-id>");
                    return Ok(false);
                };
                let handle = resolve_session_reference(target)?;
                let session = Session::load_from_path(&handle.path)?;
                let message_count = session.messages.len();
                self.runtime = build_runtime(
                    session,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                    self.telemetry.clone(),
                )?;
                self.session = handle;
                println!(
                    "Session switched\n  Active session   {}\n  File             {}\n  Messages         {}",
                    self.session.id,
                    self.session.path.display(),
                    message_count,
                );
                Ok(true)
            }
            Some(other) => {
                println!("Unknown /session action '{other}'. Use /session list or /session switch <session-id>.");
                Ok(false)
            }
        }
    }

    fn handle_plugins_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        let loader = ConfigLoader::default_for(&cwd);
        let runtime_config = loader.load()?;
        let mut manager = build_plugin_manager(&cwd, &loader, &runtime_config);
        let result = handle_plugins_slash_command(action, target, &mut manager, &runtime::EprintlnReporter::new())?;
        println!("{}", result.message);
        if result.reload_runtime {
            self.reload_runtime_features()?;
        }
        Ok(false)
    }

    fn reload_runtime_features(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime = build_runtime(
            self.runtime.session().clone(),
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
            self.telemetry.clone(),
        )?;
        self.persist_session()
    }

    fn compact(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let result = self.runtime.compact(CompactionConfig::default());
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let skipped = removed == 0;
        self.runtime = build_runtime(
            result.compacted_session,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
            self.telemetry.clone(),
        )?;
        self.persist_session()?;
        println!("{}", format_compact_report(removed, kept, skipped));
        Ok(())
    }

    fn run_internal_prompt_text_with_progress(
        &self,
        prompt: &str,
        enable_tools: bool,
        progress: Option<InternalPromptProgressReporter>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let session = self.runtime.session().clone();
        let mut runtime = build_runtime(
            session,
            self.model.clone(),
            self.system_prompt.clone(),
            enable_tools,
            false,
            self.allowed_tools.clone(),
            self.permission_mode,
            progress,
            self.telemetry.clone(),
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let summary = runtime.run_turn(prompt, Some(&mut permission_prompter))?;
        Ok(final_assistant_text(&summary).trim().to_string())
    }

    fn run_internal_prompt_text(
        &self,
        prompt: &str,
        enable_tools: bool,
    ) -> Result<String, Box<dyn std::error::Error>> {
        self.run_internal_prompt_text_with_progress(prompt, enable_tools, None)
    }

    fn run_bughunter(&self, scope: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let scope = scope.unwrap_or("the current repository");
        let prompt = format!(
            "You are /bughunter. Inspect {scope} and identify the most likely bugs or correctness issues. Prioritize concrete findings with file paths, severity, and suggested fixes. Use tools if needed."
        );
        println!("{}", self.run_internal_prompt_text(&prompt, true)?);
        Ok(())
    }

    fn run_ultraplan(&self, task: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let task = task.unwrap_or("the current repo work");
        let prompt = format!(
            "You are /ultraplan. Produce a deep multi-step execution plan for {task}. Include goals, risks, implementation sequence, verification steps, and rollback considerations. Use tools if needed."
        );
        let mut progress = InternalPromptProgressRun::start_ultraplan(task);
        match self.run_internal_prompt_text_with_progress(&prompt, true, Some(progress.reporter()))
        {
            Ok(plan) => {
                progress.finish_success();
                println!("{plan}");
                Ok(())
            }
            Err(error) => {
                progress.finish_failure(&error.to_string());
                Err(error)
            }
        }
    }

    #[allow(clippy::unused_self)]
    fn run_teleport(&self, target: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let Some(target) = target.map(str::trim).filter(|value| !value.is_empty()) else {
            println!("Usage: /teleport <symbol-or-path>");
            return Ok(());
        };

        println!("{}", render_teleport_report(target)?);
        Ok(())
    }

    fn run_debug_tool_call(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_last_tool_debug_report(self.runtime.session())?);
        Ok(())
    }

    fn run_commit(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let status = git_output(&["status", "--short"])?;
        if status.trim().is_empty() {
            println!("Commit\n  Result           skipped\n  Reason           no workspace changes");
            return Ok(());
        }

        git_status_ok(&["add", "-A"])?;
        let staged_stat = git_output(&["diff", "--cached", "--stat"])?;
        let prompt = format!(
            "Generate a git commit message in plain text Lore format only. Base it on this staged diff summary:\n\n{}\n\nRecent conversation context:\n{}",
            truncate_for_prompt(&staged_stat, 8_000),
            recent_user_context(self.runtime.session(), 6)
        );
        let message = sanitize_generated_message(&self.run_internal_prompt_text(&prompt, false)?);
        if message.trim().is_empty() {
            return Err("generated commit message was empty".into());
        }

        let path = write_temp_text_file("elai-commit-message.txt", &message)?;
        let output = Command::new("git")
            .args(["commit", "--file"])
            .arg(&path)
            .current_dir(env::current_dir()?)
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!("git commit failed: {stderr}").into());
        }

        println!(
            "Commit\n  Result           created\n  Message file     {}\n\n{}",
            path.display(),
            message.trim()
        );
        Ok(())
    }

    fn run_pr(&self, context: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let staged = git_output(&["diff", "--stat"])?;
        let prompt = format!(
            "Generate a pull request title and body from this conversation and diff summary. Output plain text in this format exactly:\nTITLE: <title>\nBODY:\n<body markdown>\n\nContext hint: {}\n\nDiff summary:\n{}",
            context.unwrap_or("none"),
            truncate_for_prompt(&staged, 10_000)
        );
        let draft = sanitize_generated_message(&self.run_internal_prompt_text(&prompt, false)?);
        let (title, body) = parse_titled_body(&draft)
            .ok_or_else(|| "failed to parse generated PR title/body".to_string())?;

        if command_exists("gh") {
            let body_path = write_temp_text_file("elai-pr-body.md", &body)?;
            let output = Command::new("gh")
                .args(["pr", "create", "--title", &title, "--body-file"])
                .arg(&body_path)
                .current_dir(env::current_dir()?)
                .output()?;
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                println!(
                    "PR\n  Result           created\n  Title            {title}\n  URL              {}",
                    if stdout.is_empty() { "<unknown>" } else { &stdout }
                );
                return Ok(());
            }
        }

        println!("PR draft\n  Title            {title}\n\n{body}");
        Ok(())
    }

    fn run_issue(&self, context: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let prompt = format!(
            "Generate a GitHub issue title and body from this conversation. Output plain text in this format exactly:\nTITLE: <title>\nBODY:\n<body markdown>\n\nContext hint: {}\n\nConversation context:\n{}",
            context.unwrap_or("none"),
            truncate_for_prompt(&recent_user_context(self.runtime.session(), 10), 10_000)
        );
        let draft = sanitize_generated_message(&self.run_internal_prompt_text(&prompt, false)?);
        let (title, body) = parse_titled_body(&draft)
            .ok_or_else(|| "failed to parse generated issue title/body".to_string())?;

        if command_exists("gh") {
            let body_path = write_temp_text_file("elai-issue-body.md", &body)?;
            let output = Command::new("gh")
                .args(["issue", "create", "--title", &title, "--body-file"])
                .arg(&body_path)
                .current_dir(env::current_dir()?)
                .output()?;
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                println!(
                    "Issue\n  Result           created\n  Title            {title}\n  URL              {}",
                    if stdout.is_empty() { "<unknown>" } else { &stdout }
                );
                return Ok(());
            }
        }

        println!("Issue draft\n  Title            {title}\n\n{body}");
        Ok(())
    }

    fn run_dream(&self, force: bool) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        let model_call = |prompt: &str| self.run_internal_prompt_text(prompt, false);
        let outcome = runtime::with_task_default(
            runtime::TaskType::Dream,
            "elai dream",
            "Dream",
            None,
            |reporter| dream::execute_dream(&cwd, force, &model_call, reporter),
        )?;
        if let Some(result) = outcome {
            println!("{}", dream::format_dream_output(&result));
        }
        Ok(())
    }
}

impl Drop for LiveCli {
    fn drop(&mut self) {
        // Silently flush cache on exit; errors are non-critical.
        let _ = self.cache.flush();
    }
}

fn sessions_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let path = cwd.join(".elai").join("sessions");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn create_managed_session_handle() -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let id = generate_session_id();
    let path = sessions_dir()?.join(format!("{id}.json"));
    Ok(SessionHandle { id, path })
}

fn generate_session_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("session-{millis}")
}

fn resolve_session_reference(reference: &str) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let direct = PathBuf::from(reference);
    let path = if direct.exists() {
        direct
    } else {
        sessions_dir()?.join(format!("{reference}.json"))
    };
    if !path.exists() {
        return Err(format!("session not found: {reference}").into());
    }
    let id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(reference)
        .to_string();
    Ok(SessionHandle { id, path })
}

fn list_managed_sessions() -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    let mut sessions = Vec::new();
    for entry in fs::read_dir(sessions_dir()?)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified_epoch_secs = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let message_count = Session::load_from_path(&path)
            .map(|session| session.messages.len())
            .unwrap_or_default();
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string();
        sessions.push(ManagedSessionSummary {
            id,
            path,
            modified_epoch_secs,
            message_count,
        });
    }
    sessions.sort_by(|left, right| right.modified_epoch_secs.cmp(&left.modified_epoch_secs));
    Ok(sessions)
}

fn render_session_list(active_session_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let sessions = list_managed_sessions()?;
    let mut lines = vec![
        "Sessions".to_string(),
        format!("  Directory         {}", sessions_dir()?.display()),
    ];
    if sessions.is_empty() {
        lines.push("  No managed sessions saved yet.".to_string());
        return Ok(lines.join("\n"));
    }
    for session in sessions {
        let marker = if session.id == active_session_id {
            "● current"
        } else {
            "○ saved"
        };
        lines.push(format!(
            "  {id:<20} {marker:<10} msgs={msgs:<4} modified={modified} path={path}",
            id = session.id,
            msgs = session.message_count,
            modified = session.modified_epoch_secs,
            path = session.path.display(),
        ));
    }
    Ok(lines.join("\n"))
}

fn render_repl_help() -> String {
    [
        "REPL".to_string(),
        "  /exit                Quit the REPL".to_string(),
        "  /quit                Quit the REPL".to_string(),
        "  /vim                 Toggle Vim keybindings".to_string(),
        "  Up/Down              Navigate prompt history".to_string(),
        "  Tab                  Complete slash commands".to_string(),
        "  Ctrl-C               Clear input (or exit on empty prompt)".to_string(),
        "  Shift+Enter/Ctrl+J   Insert a newline".to_string(),
        String::new(),
        render_slash_command_help(),
    ]
    .join(
        "
",
    )
}

fn status_context(
    session_path: Option<&Path>,
) -> Result<StatusContext, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let discovered_config_files = loader.discover().len();
    let runtime_config = loader.load()?;
    let project_context = ProjectContext::discover_with_git(&cwd, DEFAULT_DATE)?;
    let (project_root, git_branch) =
        parse_git_status_metadata(project_context.git_status.as_deref());
    Ok(StatusContext {
        cwd,
        session_path: session_path.map(Path::to_path_buf),
        loaded_config_files: runtime_config.loaded_entries().len(),
        discovered_config_files,
        memory_file_count: project_context.instruction_files.len(),
        project_root,
        git_branch,
    })
}

fn format_status_report(
    model: &str,
    usage: StatusUsage,
    permission_mode: &str,
    context: &StatusContext,
) -> String {
    [
        format!(
            "Status
  Model            {model}
  Permission mode  {permission_mode}
  Messages         {}
  Turns            {}
  Estimated tokens {}",
            usage.message_count, usage.turns, usage.estimated_tokens,
        ),
        format!(
            "Usage
  Latest total     {}
  Cumulative input {}
  Cumulative output {}
  Cumulative total {}",
            usage.latest.total_tokens(),
            usage.cumulative.input_tokens,
            usage.cumulative.output_tokens,
            usage.cumulative.total_tokens(),
        ),
        format!(
            "Workspace
  Cwd              {}
  Project root     {}
  Git branch       {}
  Session          {}
  Config files     loaded {}/{}
  Memory files     {}",
            context.cwd.display(),
            context
                .project_root
                .as_ref()
                .map_or_else(|| "unknown".to_string(), |path| path.display().to_string()),
            context.git_branch.as_deref().unwrap_or("unknown"),
            context.session_path.as_ref().map_or_else(
                || "live-repl".to_string(),
                |path| path.display().to_string()
            ),
            context.loaded_config_files,
            context.discovered_config_files,
            context.memory_file_count,
        ),
    ]
    .join(
        "

",
    )
}

fn render_config_report(section: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let discovered = loader.discover();
    let runtime_config = loader.load()?;

    let mut lines = vec![
        format!(
            "Config
  Working directory {}
  Loaded files      {}
  Merged keys       {}",
            cwd.display(),
            runtime_config.loaded_entries().len(),
            runtime_config.merged().len()
        ),
        "Discovered files".to_string(),
    ];
    for entry in discovered {
        let source = match entry.source {
            ConfigSource::User => "user",
            ConfigSource::Project => "project",
            ConfigSource::Local => "local",
        };
        let status = if runtime_config
            .loaded_entries()
            .iter()
            .any(|loaded_entry| loaded_entry.path == entry.path)
        {
            "loaded"
        } else {
            "missing"
        };
        lines.push(format!(
            "  {source:<7} {status:<7} {}",
            entry.path.display()
        ));
    }

    if let Some(section) = section {
        lines.push(format!("Merged section: {section}"));
        let value = match section {
            "env" => runtime_config.get("env"),
            "hooks" => runtime_config.get("hooks"),
            "model" => runtime_config.get("model"),
            "plugins" => runtime_config
                .get("plugins")
                .or_else(|| runtime_config.get("enabledPlugins")),
            other => {
                lines.push(format!(
                    "  Unsupported config section '{other}'. Use env, hooks, model, or plugins."
                ));
                return Ok(lines.join(
                    "
",
                ));
            }
        };
        lines.push(format!(
            "  {}",
            match value {
                Some(value) => value.render(),
                None => "<unset>".to_string(),
            }
        ));
        return Ok(lines.join(
            "
",
        ));
    }

    lines.push("Merged JSON".to_string());
    lines.push(format!("  {}", runtime_config.as_json().render()));
    Ok(lines.join(
        "
",
    ))
}

fn render_memory_report() -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let project_context = ProjectContext::discover(&cwd, DEFAULT_DATE)?;
    let mut lines = vec![format!(
        "Memory
  Working directory {}
  Instruction files {}",
        cwd.display(),
        project_context.instruction_files.len()
    )];
    if project_context.instruction_files.is_empty() {
        lines.push("Discovered files".to_string());
        lines.push(
            "  No ELAI instruction files discovered in the current directory ancestry.".to_string(),
        );
    } else {
        lines.push("Discovered files".to_string());
        for (index, file) in project_context.instruction_files.iter().enumerate() {
            let preview = file.content.lines().next().unwrap_or("").trim();
            let preview = if preview.is_empty() {
                "<empty>"
            } else {
                preview
            };
            lines.push(format!("  {}. {}", index + 1, file.path.display(),));
            lines.push(format!(
                "     lines={} preview={}",
                file.content.lines().count(),
                preview
            ));
        }
    }
    Ok(lines.join(
        "
",
    ))
}

fn init_elai_md() -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    Ok(initialize_repo(&cwd, &crate::args::InitArgs::default())?.render())
}

fn run_init(args: &crate::args::InitArgs) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let report = init::initialize_repo(&cwd, args)?;
    println!("{}", report.render());
    Ok(())
}

fn parse_init_args(rest: &[String]) -> crate::args::InitArgs {
    use crate::args::{EmbedProviderArg, IndexBackend, InitArgs};
    let mut args = InitArgs::default();
    let mut idx = 0;
    while idx < rest.len() {
        match rest[idx].as_str() {
            "--no-index" => { args.no_index = true; idx += 1; }
            "--no-watcher" => { args.no_watcher = true; idx += 1; }
            "--reindex" => { args.reindex = true; idx += 1; }
            "--backend" => {
                if let Some(v) = rest.get(idx + 1) {
                    args.backend = match v.as_str() {
                        "qdrant" => IndexBackend::Qdrant,
                        _ => IndexBackend::Sqlite,
                    };
                    idx += 2;
                } else { idx += 1; }
            }
            "--embed-provider" => {
                if let Some(v) = rest.get(idx + 1) {
                    args.embed_provider = match v.as_str() {
                        "ollama" => EmbedProviderArg::Ollama,
                        "jina" => EmbedProviderArg::Jina,
                        "openai" => EmbedProviderArg::Openai,
                        "voyage" => EmbedProviderArg::Voyage,
                        _ => EmbedProviderArg::Local,
                    };
                    idx += 2;
                } else { idx += 1; }
            }
            "--embed-model" => {
                if let Some(v) = rest.get(idx + 1) { args.embed_model = Some(v.clone()); idx += 2; } else { idx += 1; }
            }
            "--ollama-url" => {
                if let Some(v) = rest.get(idx + 1) { args.ollama_url = Some(v.clone()); idx += 2; } else { idx += 1; }
            }
            "--qdrant-url" => {
                if let Some(v) = rest.get(idx + 1) { args.qdrant_url = Some(v.clone()); idx += 2; } else { idx += 1; }
            }
            _ => { idx += 1; }
        }
    }
    args
}

fn normalize_permission_mode(mode: &str) -> Option<&'static str> {
    match mode.trim() {
        "read-only" => Some("read-only"),
        "workspace-write" => Some("workspace-write"),
        "danger-full-access" => Some("danger-full-access"),
        _ => None,
    }
}

fn render_diff_report() -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--", ":(exclude).omx"])
        .current_dir(env::current_dir()?)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git diff failed: {stderr}").into());
    }
    let diff = String::from_utf8(output.stdout)?;
    if diff.trim().is_empty() {
        return Ok(
            "Diff\n  Result           clean working tree\n  Detail           no current changes"
                .to_string(),
        );
    }
    Ok(format!("Diff\n\n{}", diff.trim_end()))
}

fn render_teleport_report(target: &str) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;

    let file_list = Command::new("rg")
        .args(["--files"])
        .current_dir(&cwd)
        .output()?;
    let file_matches = if file_list.status.success() {
        String::from_utf8(file_list.stdout)?
            .lines()
            .filter(|line| line.contains(target))
            .take(10)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let content_output = Command::new("rg")
        .args(["-n", "-S", "--color", "never", target, "."])
        .current_dir(&cwd)
        .output()?;

    let mut lines = vec![format!("Teleport\n  Target           {target}")];
    if !file_matches.is_empty() {
        lines.push(String::new());
        lines.push("File matches".to_string());
        lines.extend(file_matches.into_iter().map(|path| format!("  {path}")));
    }

    if content_output.status.success() {
        let matches = String::from_utf8(content_output.stdout)?;
        if !matches.trim().is_empty() {
            lines.push(String::new());
            lines.push("Content matches".to_string());
            lines.push(truncate_for_prompt(&matches, 4_000));
        }
    }

    if lines.len() == 1 {
        lines.push("  Result           no matches found".to_string());
    }

    Ok(lines.join("\n"))
}

fn render_last_tool_debug_report(session: &Session) -> Result<String, Box<dyn std::error::Error>> {
    let last_tool_use = session
        .messages
        .iter()
        .rev()
        .find_map(|message| {
            message.blocks.iter().rev().find_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
        })
        .ok_or_else(|| "no prior tool call found in session".to_string())?;

    let tool_result = session.messages.iter().rev().find_map(|message| {
        message.blocks.iter().rev().find_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } if tool_use_id == &last_tool_use.0 => {
                Some((tool_name.clone(), output.clone(), *is_error))
            }
            _ => None,
        })
    });

    let mut lines = vec![
        "Debug tool call".to_string(),
        format!("  Tool id          {}", last_tool_use.0),
        format!("  Tool name        {}", last_tool_use.1),
        "  Input".to_string(),
        indent_block(&last_tool_use.2, 4),
    ];

    match tool_result {
        Some((tool_name, output, is_error)) => {
            lines.push("  Result".to_string());
            lines.push(format!("    name           {tool_name}"));
            lines.push(format!(
                "    status         {}",
                if is_error { "error" } else { "ok" }
            ));
            lines.push(indent_block(&output, 4));
        }
        None => lines.push("  Result           missing tool result".to_string()),
    }

    Ok(lines.join("\n"))
}

fn indent_block(value: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn git_output(args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env::current_dir()?)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")).into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn git_status_ok(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env::current_dir()?)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {} failed: {stderr}", args.join(" ")).into());
    }
    Ok(())
}

fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn write_temp_text_file(
    filename: &str,
    contents: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = env::temp_dir().join(filename);
    fs::write(&path, contents)?;
    Ok(path)
}

fn recent_user_context(session: &Session, limit: usize) -> String {
    let requests = session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .filter_map(|message| {
            message.blocks.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.trim().to_string()),
                _ => None,
            })
        })
        .rev()
        .take(limit)
        .collect::<Vec<_>>();

    if requests.is_empty() {
        "<no prior user messages>".to_string()
    } else {
        requests
            .into_iter()
            .rev()
            .enumerate()
            .map(|(index, text)| format!("{}. {}", index + 1, text))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn truncate_for_prompt(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.trim().to_string()
    } else {
        let truncated = value.chars().take(limit).collect::<String>();
        format!("{}\n…[truncated]", truncated.trim_end())
    }
}

fn sanitize_generated_message(value: &str) -> String {
    value.trim().trim_matches('`').trim().replace("\r\n", "\n")
}

fn parse_titled_body(value: &str) -> Option<(String, String)> {
    let normalized = sanitize_generated_message(value);
    let title = normalized
        .lines()
        .find_map(|line| line.strip_prefix("TITLE:").map(str::trim))?;
    let body_start = normalized.find("BODY:")?;
    let body = normalized[body_start + "BODY:".len()..].trim();
    Some((title.to_string(), body.to_string()))
}

fn render_version_report() -> String {
    let git_sha = GIT_SHA.unwrap_or("unknown");
    let target = BUILD_TARGET.unwrap_or("unknown");
    format!(
        "Elai Code\n  Version          {VERSION}\n  Git SHA          {git_sha}\n  Target           {target}\n  Build date       {DEFAULT_DATE}"
    )
}

fn render_export_text(session: &Session) -> String {
    let mut lines = vec!["# Conversation Export".to_string(), String::new()];
    for (index, message) in session.messages.iter().enumerate() {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        lines.push(format!("## {}. {role}", index + 1));
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => lines.push(text.clone()),
                ContentBlock::ToolUse { id, name, input } => {
                    lines.push(format!("[tool_use id={id} name={name}] {input}"));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    tool_name,
                    output,
                    is_error,
                } => {
                    lines.push(format!(
                        "[tool_result id={tool_use_id} name={tool_name} error={is_error}] {output}"
                    ));
                }
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

fn default_export_filename(session: &Session) -> String {
    let stem = session
        .messages
        .iter()
        .find_map(|message| match message.role {
            MessageRole::User => message.blocks.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }),
            _ => None,
        })
        .map_or("conversation", |text| {
            text.lines().next().unwrap_or("conversation")
        })
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let fallback = if stem.is_empty() {
        "conversation"
    } else {
        &stem
    };
    format!("{fallback}.txt")
}

fn resolve_export_path(
    requested_path: Option<&str>,
    session: &Session,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let file_name =
        requested_path.map_or_else(|| default_export_filename(session), ToOwned::to_owned);
    let final_name = if Path::new(&file_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
    {
        file_name
    } else {
        format!("{file_name}.txt")
    };
    Ok(cwd.join(final_name))
}

fn build_system_prompt() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(load_system_prompt(
        env::current_dir()?,
        DEFAULT_DATE,
        env::consts::OS,
        "unknown",
    )?)
}

/// Enriquece o input do usuário com conteúdo de arquivos mencionados via `@<path>`.
/// Retorna o input original concatenado com cada arquivo lido em bloco fenced.
/// Se não houver menções válidas, retorna o input original sem mudanças.
fn enrich_input_with_mentions(input: &str, cwd: &Path) -> String {
    let mentions = runtime::parse_mentions(input);
    if mentions.is_empty() {
        return input.to_string();
    }
    let files = runtime::read_mentioned_files(cwd, &mentions);
    if files.is_empty() {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len() + 4096);
    out.push_str(input);
    out.push_str("\n\n---\n\n# Mentioned files\n\n");
    for f in &files {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("## {}\n\n```\n{}\n```\n\n", f.path.display(), f.content),
        );
    }
    out
}

fn build_runtime_plugin_state(
) -> Result<(runtime::RuntimeFeatureConfig, GlobalToolRegistry), Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader.load()?;
    let plugin_manager = build_plugin_manager(&cwd, &loader, &runtime_config);
    let mut tool_registry =
        GlobalToolRegistry::with_plugin_tools(plugin_manager.aggregated_tools()?)?;

    // Wire MCP tools into the registry so the LLM sees them in definitions().
    // discover_tools() is async; we use a temporary single-threaded runtime to
    // drive it synchronously here (the caller is non-async).
    let mut mcp_manager = McpServerManager::from_runtime_config(&runtime_config);
    if !mcp_manager.unsupported_servers().is_empty() {
        for unsupported in mcp_manager.unsupported_servers() {
            eprintln!(
                "[elai] MCP server '{}' skipped: {}",
                unsupported.server_name, unsupported.reason
            );
        }
    }
    let mcp_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    if let Err(e) = mcp_rt.block_on(mcp_manager.discover_tools()) {
        eprintln!("[elai] MCP tool discovery failed (continuing without MCP tools): {e}");
    }
    let mcp_manager_arc = Arc::new(Mutex::new(mcp_manager));
    let mcp_source = McpToolSource::new(Arc::clone(&mcp_manager_arc));
    tool_registry.add_source(Box::new(mcp_source));

    // Initialise session-scoped rate limiter from catalog overrides.
    {
        use runtime::{init_rate_limiter, RateLimit, ToolCatalog};
        let catalog = ToolCatalog::load(&cwd);
        let rate_overrides: std::collections::HashMap<String, RateLimit> = catalog
            .overrides
            .iter()
            .filter_map(|o| catalog.rate_limit_for(&o.id).map(|rl| (o.id.clone(), rl)))
            .collect();
        init_rate_limiter(rate_overrides);
    }

    Ok((runtime_config.feature_config().clone(), tool_registry))
}

fn build_plugin_manager(
    cwd: &Path,
    loader: &ConfigLoader,
    runtime_config: &runtime::RuntimeConfig,
) -> PluginManager {
    let plugin_settings = runtime_config.plugins();
    let mut plugin_config = PluginManagerConfig::new(loader.config_home().to_path_buf());
    plugin_config.enabled_plugins = plugin_settings.enabled_plugins().clone();
    plugin_config.external_dirs = plugin_settings
        .external_directories()
        .iter()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path))
        .collect();
    plugin_config.install_root = plugin_settings
        .install_root()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    plugin_config.registry_path = plugin_settings
        .registry_path()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    plugin_config.bundled_root = plugin_settings
        .bundled_root()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    PluginManager::new(plugin_config)
}

fn resolve_plugin_path(cwd: &Path, config_home: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else if value.starts_with('.') {
        cwd.join(path)
    } else {
        config_home.join(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InternalPromptProgressState {
    command_label: &'static str,
    task_label: String,
    step: usize,
    phase: String,
    detail: Option<String>,
    saw_final_text: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InternalPromptProgressEvent {
    Started,
    Update,
    Heartbeat,
    Complete,
    Failed,
}

#[derive(Debug)]
struct InternalPromptProgressShared {
    state: Mutex<InternalPromptProgressState>,
    output_lock: Mutex<()>,
    started_at: Instant,
}

#[derive(Debug, Clone)]
struct InternalPromptProgressReporter {
    shared: Arc<InternalPromptProgressShared>,
}

#[derive(Debug)]
struct InternalPromptProgressRun {
    reporter: InternalPromptProgressReporter,
    heartbeat_stop: Option<mpsc::Sender<()>>,
    heartbeat_handle: Option<thread::JoinHandle<()>>,
}

impl InternalPromptProgressReporter {
    fn ultraplan(task: &str) -> Self {
        Self {
            shared: Arc::new(InternalPromptProgressShared {
                state: Mutex::new(InternalPromptProgressState {
                    command_label: "Ultraplan",
                    task_label: task.to_string(),
                    step: 0,
                    phase: "planning started".to_string(),
                    detail: Some(format!("task: {task}")),
                    saw_final_text: false,
                }),
                output_lock: Mutex::new(()),
                started_at: Instant::now(),
            }),
        }
    }

    fn emit(&self, event: InternalPromptProgressEvent, error: Option<&str>) {
        let snapshot = self.snapshot();
        let line = format_internal_prompt_progress_line(event, &snapshot, self.elapsed(), error);
        self.write_line(&line);
    }

    fn mark_model_phase(&self) {
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            state.step += 1;
            state.phase = if state.step == 1 {
                "analyzing request".to_string()
            } else {
                "reviewing findings".to_string()
            };
            state.detail = Some(format!("task: {}", state.task_label));
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    fn mark_tool_phase(&self, name: &str, input: &str) {
        let detail = describe_tool_progress(name, input);
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            state.step += 1;
            state.phase = format!("running {name}");
            state.detail = Some(detail);
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    fn mark_text_phase(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let detail = truncate_for_summary(first_visible_line(trimmed), 120);
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            if state.saw_final_text {
                return;
            }
            state.saw_final_text = true;
            state.step += 1;
            state.phase = "drafting final plan".to_string();
            state.detail = (!detail.is_empty()).then_some(detail);
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    fn emit_heartbeat(&self) {
        let snapshot = self.snapshot();
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Heartbeat,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    fn snapshot(&self) -> InternalPromptProgressState {
        self.shared
            .state
            .lock()
            .expect("internal prompt progress state poisoned")
            .clone()
    }

    fn elapsed(&self) -> Duration {
        self.shared.started_at.elapsed()
    }

    fn write_line(&self, line: &str) {
        let _guard = self
            .shared
            .output_lock
            .lock()
            .expect("internal prompt progress output lock poisoned");
        let mut stdout = io::stdout();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }
}

impl InternalPromptProgressRun {
    fn start_ultraplan(task: &str) -> Self {
        let reporter = InternalPromptProgressReporter::ultraplan(task);
        reporter.emit(InternalPromptProgressEvent::Started, None);

        let (heartbeat_stop, heartbeat_rx) = mpsc::channel();
        let heartbeat_reporter = reporter.clone();
        let heartbeat_handle = thread::spawn(move || loop {
            match heartbeat_rx.recv_timeout(INTERNAL_PROGRESS_HEARTBEAT_INTERVAL) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => heartbeat_reporter.emit_heartbeat(),
            }
        });

        Self {
            reporter,
            heartbeat_stop: Some(heartbeat_stop),
            heartbeat_handle: Some(heartbeat_handle),
        }
    }

    fn reporter(&self) -> InternalPromptProgressReporter {
        self.reporter.clone()
    }

    fn finish_success(&mut self) {
        self.stop_heartbeat();
        self.reporter
            .emit(InternalPromptProgressEvent::Complete, None);
    }

    fn finish_failure(&mut self, error: &str) {
        self.stop_heartbeat();
        self.reporter
            .emit(InternalPromptProgressEvent::Failed, Some(error));
    }

    fn stop_heartbeat(&mut self) {
        if let Some(sender) = self.heartbeat_stop.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.heartbeat_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for InternalPromptProgressRun {
    fn drop(&mut self) {
        self.stop_heartbeat();
    }
}

fn format_internal_prompt_progress_line(
    event: InternalPromptProgressEvent,
    snapshot: &InternalPromptProgressState,
    elapsed: Duration,
    error: Option<&str>,
) -> String {
    let elapsed_seconds = elapsed.as_secs();
    let step_label = if snapshot.step == 0 {
        "current step pending".to_string()
    } else {
        format!("current step {}", snapshot.step)
    };
    let mut status_bits = vec![step_label, format!("phase {}", snapshot.phase)];
    if let Some(detail) = snapshot
        .detail
        .as_deref()
        .filter(|detail| !detail.is_empty())
    {
        status_bits.push(detail.to_string());
    }
    let status = status_bits.join(" · ");
    match event {
        InternalPromptProgressEvent::Started => {
            format!(
                "🧭 {} status · planning started · {status}",
                snapshot.command_label
            )
        }
        InternalPromptProgressEvent::Update => {
            format!("… {} status · {status}", snapshot.command_label)
        }
        InternalPromptProgressEvent::Heartbeat => format!(
            "… {} heartbeat · {elapsed_seconds}s elapsed · {status}",
            snapshot.command_label
        ),
        InternalPromptProgressEvent::Complete => format!(
            "✔ {} status · completed · {elapsed_seconds}s elapsed · {} steps total",
            snapshot.command_label, snapshot.step
        ),
        InternalPromptProgressEvent::Failed => format!(
            "✘ {} status · failed · {elapsed_seconds}s elapsed · {}",
            snapshot.command_label,
            error.unwrap_or("unknown error")
        ),
    }
}

fn describe_tool_progress(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));
    match name {
        "bash" | "Bash" => {
            let command = parsed
                .get("command")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if command.is_empty() {
                "running shell command".to_string()
            } else {
                format!("command {}", truncate_for_summary(command.trim(), 100))
            }
        }
        "read_file" | "Read" => format!("reading {}", extract_tool_path(&parsed)),
        "write_file" | "Write" => format!("writing {}", extract_tool_path(&parsed)),
        "edit_file" | "Edit" => format!("editing {}", extract_tool_path(&parsed)),
        "glob_search" | "Glob" => {
            let pattern = parsed
                .get("pattern")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let scope = parsed
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            format!("glob `{pattern}` in {scope}")
        }
        "grep_search" | "Grep" => {
            let pattern = parsed
                .get("pattern")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let scope = parsed
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            format!("grep `{pattern}` in {scope}")
        }
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|value| value.as_str())
            .map_or_else(
                || "running web search".to_string(),
                |query| format!("query {}", truncate_for_summary(query, 100)),
            ),
        _ => {
            let summary = summarize_tool_payload(input);
            if summary.is_empty() {
                format!("running {name}")
            } else {
                format!("{name}: {summary}")
            }
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
fn build_runtime(
    session: Session,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
    telemetry: TelemetryHandle,
) -> Result<ConversationRuntime<DefaultRuntimeClient, CliToolExecutor>, Box<dyn std::error::Error>>
{
    build_runtime_with_thinking(
        session,
        model,
        system_prompt,
        enable_tools,
        emit_output,
        allowed_tools,
        permission_mode,
        progress_reporter,
        telemetry,
        None,
    )
}

#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
fn build_runtime_with_thinking(
    session: Session,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
    telemetry: TelemetryHandle,
    thinking: Option<ThinkingConfig>,
) -> Result<ConversationRuntime<DefaultRuntimeClient, CliToolExecutor>, Box<dyn std::error::Error>>
{
    let (feature_config, tool_registry) = build_runtime_plugin_state()?;
    let swd_level = Arc::new(std::sync::atomic::AtomicU8::new(
        crate::swd::SwdLevel::default() as u8,
    ));
    let model_name = model.clone();
    Ok(ConversationRuntime::new_with_features(
        session,
        DefaultRuntimeClient::new(
            model,
            enable_tools,
            emit_output,
            allowed_tools.clone(),
            tool_registry.clone(),
            progress_reporter,
            Arc::clone(&swd_level),
        )?
        .with_thinking_opt(thinking),
        CliToolExecutor::new(
            allowed_tools.clone(),
            emit_output,
            tool_registry.clone(),
            Arc::clone(&swd_level),
        ),
        permission_policy(permission_mode, &tool_registry),
        system_prompt,
        &feature_config,
    )
    .with_telemetry(telemetry)
    .with_model_name(model_name))
}

#[allow(clippy::too_many_arguments)]
fn build_runtime_for_tui(
    session: Session,
    model: String,
    system_prompt: Vec<String>,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    tui_msg_tx: mpsc::Sender<tui::TuiMsg>,
    swd_level: Arc<std::sync::atomic::AtomicU8>,
    thinking_override: Option<ThinkingConfig>,
) -> Result<ConversationRuntime<DefaultRuntimeClient, CliToolExecutor>, Box<dyn std::error::Error>>
{
    let (feature_config, tool_registry) = build_runtime_plugin_state()?;
    Ok(ConversationRuntime::new_with_features(
        session,
        DefaultRuntimeClient::new(
            model,
            true,
            false, // emit_output=false; TUI gets text via channel
            allowed_tools.clone(),
            tool_registry.clone(),
            None,
            Arc::clone(&swd_level),
        )?
        .with_tui_sender(tui_msg_tx.clone())
        .with_thinking_opt(thinking_override),
        CliToolExecutor::new(
            allowed_tools.clone(),
            false,
            tool_registry.clone(),
            Arc::clone(&swd_level),
        )
        .with_tui_sender(tui_msg_tx),
        permission_policy(permission_mode, &tool_registry),
        system_prompt,
        &feature_config,
    ))
}

fn tool_whitelist_path() -> Option<std::path::PathBuf> {
    env::var("HOME").ok().map(|home| {
        std::path::PathBuf::from(home)
            .join(".config")
            .join("elai")
            .join("tool_whitelist.json")
    })
}

fn load_permanent_tool_whitelist() -> BTreeSet<String> {
    let Some(path) = tool_whitelist_path() else {
        return BTreeSet::new();
    };
    let Ok(data) = fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    serde_json::from_str::<Vec<String>>(&data)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn save_permanent_tool_whitelist(set: &BTreeSet<String>) {
    let Some(path) = tool_whitelist_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let list: Vec<&String> = set.iter().collect();
    if let Ok(data) = serde_json::to_string_pretty(&list) {
        let _ = fs::write(&path, data);
    }
}

struct CliPermissionPrompter {
    current_mode: PermissionMode,
    tui_perm_tx: Option<mpsc::Sender<tui::PermRequest>>,
    allowed_permanently: BTreeSet<String>,
}

impl CliPermissionPrompter {
    fn new(current_mode: PermissionMode) -> Self {
        Self {
            current_mode,
            tui_perm_tx: None,
            allowed_permanently: BTreeSet::new(),
        }
    }

    fn new_tui(current_mode: PermissionMode, tx: mpsc::Sender<tui::PermRequest>) -> Self {
        Self {
            current_mode,
            tui_perm_tx: Some(tx),
            allowed_permanently: load_permanent_tool_whitelist(),
        }
    }
}

impl runtime::PermissionPrompter for CliPermissionPrompter {
    fn decide(
        &mut self,
        request: &runtime::PermissionRequest,
    ) -> runtime::PermissionPromptDecision {
        // Auto-approve tools in permanent whitelist
        if self.allowed_permanently.contains(&request.tool_name) {
            return runtime::PermissionPromptDecision::Allow;
        }

        // TUI mode: send request over channel and block for decision.
        if let Some(ref tx) = self.tui_perm_tx {
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            let perm_req = tui::PermRequest {
                tool_name: request.tool_name.clone(),
                input: request.input.clone(),
                required_mode: request.required_mode.as_str().to_string(),
                reply_tx,
            };
            if tx.send(perm_req).is_ok() {
                return match reply_rx.recv() {
                    Ok(tui::PermDecision::Allow) => runtime::PermissionPromptDecision::Allow,
                    Ok(tui::PermDecision::AllowAlways) => {
                        self.allowed_permanently.insert(request.tool_name.clone());
                        save_permanent_tool_whitelist(&self.allowed_permanently);
                        runtime::PermissionPromptDecision::Allow
                    }
                    _ => runtime::PermissionPromptDecision::Deny {
                        reason: format!(
                            "tool '{}' denied by user via TUI",
                            request.tool_name
                        ),
                    },
                };
            }
        }

        // Fallback: plain terminal prompt.
        println!();
        println!("Permission approval required");
        println!("  Tool             {}", request.tool_name);
        println!("  Current mode     {}", self.current_mode.as_str());
        println!("  Required mode    {}", request.required_mode.as_str());
        println!("  Input            {}", request.input);
        print!("Approve this tool call? [y/N]: ");
        let _ = io::stdout().flush();

        let mut response = String::new();
        match io::stdin().read_line(&mut response) {
            Ok(_) => {
                let normalized = response.trim().to_ascii_lowercase();
                if matches!(normalized.as_str(), "y" | "yes") {
                    runtime::PermissionPromptDecision::Allow
                } else {
                    runtime::PermissionPromptDecision::Deny {
                        reason: format!(
                            "tool '{}' denied by user approval prompt",
                            request.tool_name
                        ),
                    }
                }
            }
            Err(error) => runtime::PermissionPromptDecision::Deny {
                reason: format!("permission approval failed: {error}"),
            },
        }
    }
}

struct DefaultRuntimeClient {
    runtime: tokio::runtime::Runtime,
    client: ProviderClient,
    model: String,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    progress_reporter: Option<InternalPromptProgressReporter>,
    tui_sender: Option<mpsc::Sender<tui::TuiMsg>>,
    swd_level: Arc<std::sync::atomic::AtomicU8>,
    correction_ctx: crate::swd::CorrectionContext,
    thinking_override: Option<ThinkingConfig>,
}

impl DefaultRuntimeClient {
    fn new(
        model: String,
        enable_tools: bool,
        emit_output: bool,
        allowed_tools: Option<AllowedToolSet>,
        tool_registry: GlobalToolRegistry,
        progress_reporter: Option<InternalPromptProgressReporter>,
        swd_level: Arc<std::sync::atomic::AtomicU8>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let orchestrate = env::var("ELAI_ORCHESTRATE")
            .ok()
            .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);
        let client = if orchestrate {
            ProviderClient::orchestrated().map_err(|error| error.to_string())?
        } else {
            ProviderClient::from_model(&model).map_err(|error| error.to_string())?
        };
        Ok(Self {
            runtime: tokio::runtime::Runtime::new()?,
            client,
            model,
            enable_tools,
            emit_output,
            allowed_tools,
            tool_registry,
            progress_reporter,
            tui_sender: None,
            swd_level,
            correction_ctx: crate::swd::CorrectionContext::new(),
            thinking_override: None,
        })
    }

    fn with_tui_sender(mut self, tx: mpsc::Sender<tui::TuiMsg>) -> Self {
        self.tui_sender = Some(tx);
        self
    }

    #[allow(dead_code)]
    fn with_thinking(mut self, config: ThinkingConfig) -> Self {
        self.thinking_override = Some(config);
        self
    }

    fn with_thinking_opt(mut self, config: Option<ThinkingConfig>) -> Self {
        self.thinking_override = config;
        self
    }
}

impl ApiClient for DefaultRuntimeClient {
    #[allow(clippy::too_many_lines)]
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if let Some(progress_reporter) = &self.progress_reporter {
            progress_reporter.mark_model_phase();
        }
        let message_request = MessageRequest {
            model: self.model.clone(),
            max_tokens: max_tokens_for_model(&self.model),
            messages: convert_messages(&request.messages),
            system: (!request.system_prompt.is_empty()).then(|| request.system_prompt.join("\n\n")),
            tools: self
                .enable_tools
                .then(|| filter_tool_specs(&self.tool_registry, self.allowed_tools.as_ref(), &runtime::ToolCatalog::default(), None)),
            tool_choice: self.enable_tools.then_some(ToolChoice::Auto),
            stream: true,
            thinking: self
                .thinking_override
                .clone()
                .or_else(|| default_thinking_config(&self.model)),
            output_config: {
                let effort = match self.thinking_override.as_ref().or_else(|| {
                    // borrow only for the match; don't store the Option
                    None::<&ThinkingConfig>
                }) {
                    Some(ThinkingConfig::Enabled { .. }) => Some(EffortLevel::High),
                    Some(ThinkingConfig::Adaptive) | None
                        if default_thinking_config(&self.model).is_some() =>
                    {
                        Some(EffortLevel::Medium)
                    }
                    _ => None,
                };
                resolve_output_config(effort)
            },
        };

        // Clone sender before moving into async block.
        let tui_sender = self.tui_sender.clone();
        let emit_output = self.emit_output;
        let swd_level_arc = Arc::clone(&self.swd_level);

        self.correction_ctx.reset();
        let correction_shared: std::sync::Arc<std::sync::Mutex<crate::swd::CorrectionContext>> =
            std::sync::Arc::new(std::sync::Mutex::new(crate::swd::CorrectionContext::new()));
        let correction_for_async = std::sync::Arc::clone(&correction_shared);

        let result = self.runtime.block_on(async {
            let mut stream = self
                .client
                .stream_message(&message_request)
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?;
            let mut stdout = io::stdout();
            let mut sink = io::sink();
            let out: &mut dyn Write = if emit_output && tui_sender.is_none() {
                &mut stdout
            } else {
                &mut sink
            };
            let renderer = TerminalRenderer::new();
            let mut markdown_stream = MarkdownStreamState::default();
            let mut events = Vec::new();
            let mut pending_tool: Option<(String, String, String)> = None;
            let mut saw_stop = false;
            let mut full_text_buf = String::new();

            while let Some(event) = stream
                .next_event()
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?
            {
                match event {
                    ApiStreamEvent::MessageStart(start) => {
                        for block in start.message.content {
                            push_output_block(block, out, &mut events, &mut pending_tool, true)?;
                        }
                    }
                    ApiStreamEvent::ContentBlockStart(start) => {
                        push_output_block(
                            start.content_block,
                            out,
                            &mut events,
                            &mut pending_tool,
                            true,
                        )?;
                    }
                    ApiStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                        ContentBlockDelta::TextDelta { text } => {
                            if !text.is_empty() {
                                if let Some(progress_reporter) = &self.progress_reporter {
                                    progress_reporter.mark_text_phase(&text);
                                }
                                {
                                    use std::sync::atomic::Ordering;
                                    if crate::swd::SwdLevel::from_u8(
                                        swd_level_arc.load(Ordering::Relaxed),
                                    ) == crate::swd::SwdLevel::Full
                                    {
                                        full_text_buf.push_str(&text);
                                    }
                                }
                                if let Some(ref tx) = tui_sender {
                                    let _ = tx.send(tui::TuiMsg::TextChunk(text.clone()));
                                } else if let Some(rendered) = markdown_stream.push(&renderer, &text) {
                                    write!(out, "{rendered}")
                                        .and_then(|()| out.flush())
                                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                                }
                                events.push(AssistantEvent::TextDelta(text));
                            }
                        }
                        ContentBlockDelta::InputJsonDelta { partial_json } => {
                            if let Some((_, _, input)) = &mut pending_tool {
                                input.push_str(&partial_json);
                            }
                        }
                        ContentBlockDelta::ThinkingDelta { thinking } => {
                            if let Some(ref tx) = tui_sender {
                                let _ = tx.send(tui::TuiMsg::ThinkingChunk(thinking));
                            }
                        }
                        ContentBlockDelta::SignatureDelta { .. } => {}
                    },
                    ApiStreamEvent::ContentBlockStop(_) => {
                        if tui_sender.is_none() {
                            if let Some(rendered) = markdown_stream.flush(&renderer) {
                                write!(out, "{rendered}")
                                    .and_then(|()| out.flush())
                                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                            }
                        }
                        if let Some((id, name, input)) = pending_tool.take() {
                            if let Some(progress_reporter) = &self.progress_reporter {
                                progress_reporter.mark_tool_phase(&name, &input);
                            }
                            if let Some(ref tx) = tui_sender {
                                let _ = tx.send(tui::TuiMsg::ToolCall { name: name.clone(), input: input.clone() });
                            } else {
                                // Display tool call now that input is fully accumulated.
                                // Modo compacto por padrão (uma linha por call); modo verboso
                                // legacy só quando `ELAI_VERBOSE_TOOLS=1`, preservando o
                                // output rico para scripts/tests que dependam dele.
                                let rendered = if cli_tools_verbose() {
                                    format!("\n{}", format_tool_call_start(&name, &input))
                                } else {
                                    format_tool_call_compact(&name, &input)
                                };
                                writeln!(out, "{rendered}")
                                    .and_then(|()| out.flush())
                                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                            }
                            events.push(AssistantEvent::ToolUse { id, name, input });
                        }
                    }
                    ApiStreamEvent::MessageDelta(delta) => {
                        if let Some(ref tx) = tui_sender {
                            let _ = tx.send(tui::TuiMsg::Usage {
                                input_tokens: delta.usage.input_tokens,
                                output_tokens: delta.usage.output_tokens,
                            });
                        }
                        events.push(AssistantEvent::Usage(TokenUsage {
                            input_tokens: delta.usage.input_tokens,
                            output_tokens: delta.usage.output_tokens,
                            cache_creation_input_tokens: 0,
                            cache_read_input_tokens: 0,
                        }));
                    }
                    ApiStreamEvent::MessageStop(_) => {
                        saw_stop = true;
                        if tui_sender.is_none() {
                            if let Some(rendered) = markdown_stream.flush(&renderer) {
                                write!(out, "{rendered}")
                                    .and_then(|()| out.flush())
                                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                            }
                        }
                        {
                            use std::sync::atomic::Ordering;
                            if crate::swd::SwdLevel::from_u8(
                                swd_level_arc.load(Ordering::Relaxed),
                            ) == crate::swd::SwdLevel::Full
                            {
                                let actions = crate::swd::parse_file_actions(&full_text_buf);
                                if !actions.is_empty() {
                                    if let Some(ref sender) = tui_sender {
                                        // Compute diffs for preview before executing.
                                        let previews: Vec<(String, Vec<crate::diff::DiffHunk>)> =
                                            actions.iter().map(|action| {
                                                let old = std::fs::read_to_string(&action.path)
                                                    .unwrap_or_default();
                                                let new = match action.operation {
                                                    crate::swd::FileOp::Write => {
                                                        action.content.as_deref().unwrap_or("").to_string()
                                                    }
                                                    crate::swd::FileOp::Delete => String::new(),
                                                };
                                                let hunks = crate::diff::compute_diff(&old, &new, 3);
                                                (action.path.clone(), hunks)
                                            }).collect();

                                        let (reply_tx, reply_rx) =
                                            std::sync::mpsc::sync_channel::<bool>(1);
                                        let _ = sender.send(tui::TuiMsg::SwdDiffPreview {
                                            actions: previews,
                                            reply_tx,
                                        });
                                        let accepted = reply_rx.recv().unwrap_or(false);

                                        if accepted {
                                            let txs = crate::swd::execute_file_actions(actions);
                                            let _ = crate::swd::append_swd_log(&txs);
                                            let has_failures = txs.iter().any(|tx| {
                                                matches!(
                                                    tx.outcome,
                                                    crate::swd::SwdOutcome::Failed { .. }
                                                        | crate::swd::SwdOutcome::Drift { .. }
                                                        | crate::swd::SwdOutcome::RolledBack
                                                )
                                            });
                                            let _ = sender.send(tui::TuiMsg::SwdBatchResult(txs.clone()));
                                            if has_failures {
                                                correction_for_async
                                                    .lock()
                                                    .unwrap()
                                                    .record_failures(&txs);
                                            }
                                        }
                                        // Rejection note already sent by TUI handler.
                                    } else {
                                        // Non-TUI mode: execute directly (no preview).
                                        let txs = crate::swd::execute_file_actions(actions);
                                        let _ = crate::swd::append_swd_log(&txs);
                                        let has_failures = txs.iter().any(|tx| {
                                            matches!(
                                                tx.outcome,
                                                crate::swd::SwdOutcome::Failed { .. }
                                                    | crate::swd::SwdOutcome::Drift { .. }
                                                    | crate::swd::SwdOutcome::RolledBack
                                            )
                                        });
                                        if has_failures {
                                            correction_for_async
                                                .lock()
                                                .unwrap()
                                                .record_failures(&txs);
                                        }
                                    }
                                }
                                full_text_buf.clear();
                            }
                        }
                        events.push(AssistantEvent::MessageStop);
                    }
                }
            }

            if !saw_stop
                && events.iter().any(|event| {
                    matches!(event, AssistantEvent::TextDelta(text) if !text.is_empty())
                        || matches!(event, AssistantEvent::ToolUse { .. })
                })
            {
                events.push(AssistantEvent::MessageStop);
            }

            if events
                .iter()
                .any(|event| matches!(event, AssistantEvent::MessageStop))
            {
                return Ok(events);
            }

            let response = self
                .client
                .send_message(&MessageRequest {
                    stream: false,
                    thinking: None,
                    output_config: None,
                    ..message_request.clone()
                })
                .await
                .map_err(|error| RuntimeError::new(error.to_string()))?;
            response_to_events(response, out)
        })?;

        // Full-mode correction loop: if failures were recorded, notify TUI and retry.
        {
            use std::sync::atomic::Ordering;
            if crate::swd::SwdLevel::from_u8(self.swd_level.load(Ordering::Relaxed))
                == crate::swd::SwdLevel::Full
            {
                let ctx = correction_shared.lock().unwrap().clone();
                if ctx.has_failures() && ctx.can_retry() {
                    let attempt = ctx.attempts;
                    let max_attempts = ctx.max_attempts;
                    if let Some(ref sender) = self.tui_sender {
                        let _ = sender.send(tui::TuiMsg::CorrectionRetry { attempt, max_attempts });
                    }
                }
                self.correction_ctx = correction_shared.lock().unwrap().clone();
            }
        }

        Ok(result)
    }
}

fn final_assistant_text(summary: &runtime::TurnSummary) -> String {
    summary
        .assistant_messages
        .last()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn collect_tool_uses(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(json!({
                "id": id,
                "name": name,
                "input": input,
            })),
            _ => None,
        })
        .collect()
}

fn collect_tool_results(summary: &runtime::TurnSummary) -> Vec<serde_json::Value> {
    summary
        .tool_results
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } => Some(json!({
                "tool_use_id": tool_use_id,
                "tool_name": tool_name,
                "output": output,
                "is_error": is_error,
            })),
            _ => None,
        })
        .collect()
}

fn slash_command_completion_candidates() -> Vec<String> {
    let mut candidates = slash_command_specs()
        .iter()
        .flat_map(|spec| {
            std::iter::once(spec.name)
                .chain(spec.aliases.iter().copied())
                .map(|name| format!("/{name}"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    candidates.push("/vim".to_string());
    candidates
}

fn format_tool_call_start(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));

    let detail = match name {
        "bash" | "Bash" => format_bash_call(&parsed),
        "read_file" | "Read" => {
            let path = extract_tool_path(&parsed);
            format!("\x1b[2m📄 Reading {path}…\x1b[0m")
        }
        "write_file" | "Write" => {
            let path = extract_tool_path(&parsed);
            let lines = parsed
                .get("content")
                .and_then(|value| value.as_str())
                .map_or(0, |content| content.lines().count());
            format!("\x1b[1;32m✏️ Writing {path}\x1b[0m \x1b[2m({lines} lines)\x1b[0m")
        }
        "edit_file" | "Edit" => {
            let path = extract_tool_path(&parsed);
            let old_value = parsed
                .get("old_string")
                .or_else(|| parsed.get("oldString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let new_value = parsed
                .get("new_string")
                .or_else(|| parsed.get("newString"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            format!(
                "\x1b[1;33m📝 Editing {path}\x1b[0m{}",
                format_patch_preview(old_value, new_value)
                    .map(|preview| format!("\n{preview}"))
                    .unwrap_or_default()
            )
        }
        "glob_search" | "Glob" => format_search_start("🔎 Glob", &parsed),
        "grep_search" | "Grep" => format_search_start("🔎 Grep", &parsed),
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|value| value.as_str())
            .unwrap_or("?")
            .to_string(),
        _ => summarize_tool_payload(input),
    };

    let border = "─".repeat(name.len() + 8);
    format!(
        "\x1b[38;5;245m╭─ \x1b[1;36m{name}\x1b[0;38;5;245m ─╮\x1b[0m\n\x1b[38;5;245m│\x1b[0m {detail}\n\x1b[38;5;245m╰{border}╯\x1b[0m"
    )
}

/// Resumo de uma linha (≤ 60 chars) do input de um tool, no formato exibido
/// pelo modo compacto do CLI não-TUI **e** pelo `ToolBatchEntry` da TUI.
/// Para `bash`/`Bash` extrai o comando; para tools de arquivo extrai o path;
/// para o resto faz truncação genérica do JSON.
pub(crate) fn tool_input_one_line(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));
    let raw = match name {
        "bash" | "Bash" => parsed
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "read_file" | "Read" | "write_file" | "Write" | "edit_file" | "Edit" => {
            extract_tool_path(&parsed)
        }
        "glob_search" | "Glob" | "grep_search" | "Grep" => parsed
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => summarize_tool_payload(input),
    };
    // Achata quebras de linha e colapsa whitespace para uma única linha visual.
    let flat = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    flat.chars().take(60).collect::<String>()
}

/// Formato compacto de tool call para o CLI não-TUI: uma única linha no padrão
/// `  ⚙ name · input`. Substitui o bloco com bordas ASCII (que ocupava 3 linhas
/// por chamada) por uma representação enxuta consistente com o `ToolBatchEntry`
/// da TUI.
fn format_tool_call_compact(name: &str, input: &str) -> String {
    let summary = tool_input_one_line(name, input);
    format!(
        "  \x1b[38;5;245m\u{2699}\x1b[0m \x1b[1;36m{name}\x1b[0m \x1b[38;5;245m\u{00b7}\x1b[0m \x1b[38;5;250m{summary}\x1b[0m"
    )
}

/// Formato compacto do resultado: `    ✓` (sucesso) ou `    ✗` (erro), sem
/// repetir o nome do tool nem mostrar o output. Output completo é exibido pelo
/// caller quando `ELAI_VERBOSE_TOOLS=1` está setado ou em caso de erro.
fn format_tool_result_compact(ok: bool) -> &'static str {
    if ok {
        "    \x1b[1;32m\u{2713}\x1b[0m"
    } else {
        "    \x1b[1;31m\u{2717}\x1b[0m"
    }
}

/// Detecta se o usuário pediu output verboso de tools no modo CLI não-TUI.
fn cli_tools_verbose() -> bool {
    std::env::var_os("ELAI_VERBOSE_TOOLS")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

fn format_tool_result(name: &str, output: &str, is_error: bool) -> String {
    let icon = if is_error {
        "\x1b[1;31m✗\x1b[0m"
    } else {
        "\x1b[1;32m✓\x1b[0m"
    };
    if is_error {
        let summary = truncate_for_summary(output.trim(), 160);
        return if summary.is_empty() {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
        } else {
            format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n\x1b[38;5;203m{summary}\x1b[0m")
        };
    }

    let parsed: serde_json::Value =
        serde_json::from_str(output).unwrap_or(serde_json::Value::String(output.to_string()));
    match name {
        "bash" | "Bash" => format_bash_result(icon, &parsed),
        "read_file" | "Read" => format_read_result(icon, &parsed),
        "write_file" | "Write" => format_write_result(icon, &parsed),
        "edit_file" | "Edit" => format_edit_result(icon, &parsed),
        "glob_search" | "Glob" => format_glob_result(icon, &parsed),
        "grep_search" | "Grep" => format_grep_result(icon, &parsed),
        _ => format_generic_tool_result(icon, name, &parsed),
    }
}

const DISPLAY_TRUNCATION_NOTICE: &str =
    "\x1b[2m… output truncated for display; full result preserved in session.\x1b[0m";
const READ_DISPLAY_MAX_LINES: usize = 80;
const READ_DISPLAY_MAX_CHARS: usize = 6_000;
const TOOL_OUTPUT_DISPLAY_MAX_LINES: usize = 60;
const TOOL_OUTPUT_DISPLAY_MAX_CHARS: usize = 4_000;

fn extract_tool_path(parsed: &serde_json::Value) -> String {
    parsed
        .get("file_path")
        .or_else(|| parsed.get("filePath"))
        .or_else(|| parsed.get("path"))
        .and_then(|value| value.as_str())
        .unwrap_or("?")
        .to_string()
}

fn format_search_start(label: &str, parsed: &serde_json::Value) -> String {
    let pattern = parsed
        .get("pattern")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let scope = parsed
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or(".");
    format!("{label} {pattern}\n\x1b[2min {scope}\x1b[0m")
}

fn format_patch_preview(old_value: &str, new_value: &str) -> Option<String> {
    if old_value.is_empty() && new_value.is_empty() {
        return None;
    }
    Some(format!(
        "\x1b[38;5;203m- {}\x1b[0m\n\x1b[38;5;70m+ {}\x1b[0m",
        truncate_for_summary(first_visible_line(old_value), 72),
        truncate_for_summary(first_visible_line(new_value), 72)
    ))
}

fn format_bash_call(parsed: &serde_json::Value) -> String {
    let command = parsed
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if command.is_empty() {
        String::new()
    } else {
        format!(
            "\x1b[48;5;236;38;5;255m $ {} \x1b[0m",
            truncate_for_summary(command, 160)
        )
    }
}

fn first_visible_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
}

fn format_bash_result(icon: &str, parsed: &serde_json::Value) -> String {
    let mut lines = vec![format!("{icon} \x1b[38;5;245mbash\x1b[0m")];
    if let Some(task_id) = parsed
        .get("backgroundTaskId")
        .and_then(|value| value.as_str())
    {
        write!(&mut lines[0], " backgrounded ({task_id})").expect("write to string");
    } else if let Some(status) = parsed
        .get("returnCodeInterpretation")
        .and_then(|value| value.as_str())
        .filter(|status| !status.is_empty())
    {
        write!(&mut lines[0], " {status}").expect("write to string");
    }

    if let Some(stdout) = parsed.get("stdout").and_then(|value| value.as_str()) {
        if !stdout.trim().is_empty() {
            lines.push(truncate_output_for_display(
                stdout,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            ));
        }
    }
    if let Some(stderr) = parsed.get("stderr").and_then(|value| value.as_str()) {
        if !stderr.trim().is_empty() {
            lines.push(format!(
                "\x1b[38;5;203m{}\x1b[0m",
                truncate_output_for_display(
                    stderr,
                    TOOL_OUTPUT_DISPLAY_MAX_LINES,
                    TOOL_OUTPUT_DISPLAY_MAX_CHARS,
                )
            ));
        }
    }

    lines.join("\n\n")
}

fn format_read_result(icon: &str, parsed: &serde_json::Value) -> String {
    let file = parsed.get("file").unwrap_or(parsed);
    let path = extract_tool_path(file);
    let start_line = file
        .get("startLine")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let num_lines = file
        .get("numLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let total_lines = file
        .get("totalLines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(num_lines);
    let content = file
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let end_line = start_line.saturating_add(num_lines.saturating_sub(1));

    format!(
        "{icon} \x1b[2m📄 Read {path} (lines {}-{} of {})\x1b[0m\n{}",
        start_line,
        end_line.max(start_line),
        total_lines,
        truncate_output_for_display(content, READ_DISPLAY_MAX_LINES, READ_DISPLAY_MAX_CHARS)
    )
}

fn format_write_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let kind = parsed
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("write");
    let line_count = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .map_or(0, |content| content.lines().count());
    format!(
        "{icon} \x1b[1;32m✏️ {} {path}\x1b[0m \x1b[2m({line_count} lines)\x1b[0m",
        if kind == "create" { "Wrote" } else { "Updated" },
    )
}

fn format_structured_patch_preview(parsed: &serde_json::Value) -> Option<String> {
    let hunks = parsed.get("structuredPatch")?.as_array()?;
    let mut preview = Vec::new();
    for hunk in hunks.iter().take(2) {
        let lines = hunk.get("lines")?.as_array()?;
        for line in lines.iter().filter_map(|value| value.as_str()).take(6) {
            match line.chars().next() {
                Some('+') => preview.push(format!("\x1b[38;5;70m{line}\x1b[0m")),
                Some('-') => preview.push(format!("\x1b[38;5;203m{line}\x1b[0m")),
                _ => preview.push(line.to_string()),
            }
        }
    }
    if preview.is_empty() {
        None
    } else {
        Some(preview.join("\n"))
    }
}

fn format_edit_result(icon: &str, parsed: &serde_json::Value) -> String {
    let path = extract_tool_path(parsed);
    let suffix = if parsed
        .get("replaceAll")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        " (replace all)"
    } else {
        ""
    };
    let preview = format_structured_patch_preview(parsed).or_else(|| {
        let old_value = parsed
            .get("oldString")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let new_value = parsed
            .get("newString")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        format_patch_preview(old_value, new_value)
    });

    match preview {
        Some(preview) => format!("{icon} \x1b[1;33m📝 Edited {path}{suffix}\x1b[0m\n{preview}"),
        None => format!("{icon} \x1b[1;33m📝 Edited {path}{suffix}\x1b[0m"),
    }
}

fn format_glob_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if filenames.is_empty() {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files")
    } else {
        format!("{icon} \x1b[38;5;245mglob_search\x1b[0m matched {num_files} files\n{filenames}")
    }
}

fn format_grep_result(icon: &str, parsed: &serde_json::Value) -> String {
    let num_matches = parsed
        .get("numMatches")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let num_files = parsed
        .get("numFiles")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let content = parsed
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let filenames = parsed
        .get("filenames")
        .and_then(|value| value.as_array())
        .map(|files| {
            files
                .iter()
                .filter_map(|value| value.as_str())
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let summary = format!(
        "{icon} \x1b[38;5;245mgrep_search\x1b[0m {num_matches} matches across {num_files} files"
    );
    if !content.trim().is_empty() {
        format!(
            "{summary}\n{}",
            truncate_output_for_display(
                content,
                TOOL_OUTPUT_DISPLAY_MAX_LINES,
                TOOL_OUTPUT_DISPLAY_MAX_CHARS,
            )
        )
    } else if !filenames.is_empty() {
        format!("{summary}\n{filenames}")
    } else {
        summary
    }
}

fn format_generic_tool_result(icon: &str, name: &str, parsed: &serde_json::Value) -> String {
    let rendered_output = match parsed {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(parsed).unwrap_or_else(|_| parsed.to_string())
        }
        _ => parsed.to_string(),
    };
    let preview = truncate_output_for_display(
        &rendered_output,
        TOOL_OUTPUT_DISPLAY_MAX_LINES,
        TOOL_OUTPUT_DISPLAY_MAX_CHARS,
    );

    if preview.is_empty() {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m")
    } else if preview.contains('\n') {
        format!("{icon} \x1b[38;5;245m{name}\x1b[0m\n{preview}")
    } else {
        format!("{icon} \x1b[38;5;245m{name}:\x1b[0m {preview}")
    }
}

fn summarize_tool_payload(payload: &str) -> String {
    let compact = match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(value) => value.to_string(),
        Err(_) => payload.trim().to_string(),
    };
    truncate_for_summary(&compact, 96)
}

fn truncate_for_summary(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn truncate_output_for_display(content: &str, max_lines: usize, max_chars: usize) -> String {
    let original = content.trim_end_matches('\n');
    if original.is_empty() {
        return String::new();
    }

    let mut preview_lines = Vec::new();
    let mut used_chars = 0usize;
    let mut truncated = false;

    for (index, line) in original.lines().enumerate() {
        if index >= max_lines {
            truncated = true;
            break;
        }

        let newline_cost = usize::from(!preview_lines.is_empty());
        let available = max_chars.saturating_sub(used_chars + newline_cost);
        if available == 0 {
            truncated = true;
            break;
        }

        let line_chars = line.chars().count();
        if line_chars > available {
            preview_lines.push(line.chars().take(available).collect::<String>());
            truncated = true;
            break;
        }

        preview_lines.push(line.to_string());
        used_chars += newline_cost + line_chars;
    }

    let mut preview = preview_lines.join("\n");
    if truncated {
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(DISPLAY_TRUNCATION_NOTICE);
    }
    preview
}

fn push_output_block(
    block: OutputContentBlock,
    out: &mut (impl Write + ?Sized),
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
    streaming_tool_input: bool,
) -> Result<(), RuntimeError> {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                let rendered = TerminalRenderer::new().markdown_to_ansi(&text);
                write!(out, "{rendered}")
                    .and_then(|()| out.flush())
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            // During streaming, the initial content_block_start has an empty input ({}).
            // The real input arrives via input_json_delta events. In
            // non-streaming responses, preserve a legitimate empty object.
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
                String::new()
            } else {
                input.to_string()
            };
            *pending_tool = Some((id, name, initial_input));
        }
        OutputContentBlock::Thinking { .. } | OutputContentBlock::RedactedThinking { .. } => {}
    }
    Ok(())
}

fn response_to_events(
    response: MessageResponse,
    out: &mut (impl Write + ?Sized),
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let mut events = Vec::new();
    let mut pending_tool = None;

    for block in response.content {
        push_output_block(block, out, &mut events, &mut pending_tool, false)?;
        if let Some((id, name, input)) = pending_tool.take() {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }

    events.push(AssistantEvent::Usage(TokenUsage {
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
        cache_read_input_tokens: response.usage.cache_read_input_tokens,
    }));
    events.push(AssistantEvent::MessageStop);
    Ok(events)
}

const SWD_WRITE_TOOLS: &[&str] = &["write_file", "edit_file", "NotebookEdit"];

struct CliToolExecutor {
    renderer: TerminalRenderer,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    tui_sender: Option<mpsc::Sender<tui::TuiMsg>>,
    swd_level: Arc<std::sync::atomic::AtomicU8>,
    swd_retry_counts: std::collections::HashMap<String, u8>,
}

impl CliToolExecutor {
    fn new(
        allowed_tools: Option<AllowedToolSet>,
        emit_output: bool,
        tool_registry: GlobalToolRegistry,
        swd_level: Arc<std::sync::atomic::AtomicU8>,
    ) -> Self {
        Self {
            renderer: TerminalRenderer::new(),
            emit_output,
            allowed_tools,
            tool_registry,
            tui_sender: None,
            swd_level,
            swd_retry_counts: std::collections::HashMap::new(),
        }
    }

    fn with_tui_sender(mut self, tx: mpsc::Sender<tui::TuiMsg>) -> Self {
        self.tui_sender = Some(tx);
        self
    }

    fn execute_with_swd(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        use std::sync::atomic::Ordering;
        use std::time::{SystemTime, UNIX_EPOCH};
        use crate::swd::{self, SwdOutcome, SwdTransaction};

        let _ = self.swd_level.load(Ordering::Relaxed);

        // Extract path from JSON input.
        let path = serde_json::from_str::<serde_json::Value>(input)
            .ok()
            .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(str::to_string))
            .unwrap_or_default();

        let (before_hash, before_bytes) = if path.is_empty() {
            (None, None)
        } else {
            swd::snapshot(&path)
        };

        let value = serde_json::from_str(input)
            .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
        let result = self.tool_registry.execute(tool_name, &value);
        let tool_ok = result.is_ok();

        let (after_hash, _) = if path.is_empty() {
            (None, None)
        } else {
            swd::snapshot(&path)
        };
        let outcome = swd::verify_outcome(&before_hash, &after_hash, tool_ok);

        // Rollback if failed.
        if matches!(outcome, SwdOutcome::Failed { .. } | SwdOutcome::Drift { .. }) && !path.is_empty() {
            let _ = swd::rollback(&path, before_bytes.as_deref());
        }

        // Rich error with retry hint for partial mode correction.
        if matches!(outcome, SwdOutcome::Failed { .. } | SwdOutcome::Drift { .. }) && !path.is_empty() {
            let retry_count = self.swd_retry_counts.entry(path.clone()).or_insert(0);
            let detail = match &outcome {
                SwdOutcome::Failed { reason } => reason.clone(),
                SwdOutcome::Drift { detail } => detail.clone(),
                _ => String::new(),
            };
            if *retry_count < crate::swd::MAX_CORRECTION_ATTEMPTS {
                *retry_count += 1;
                let ts2 = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                    .unwrap_or(0);
                let tx_err = SwdTransaction {
                    tool_name: tool_name.to_string(),
                    path: path.clone(),
                    before_hash: before_hash.clone(),
                    after_hash: after_hash.clone(),
                    outcome: outcome.clone(),
                    timestamp_ms: ts2,
                };
                let _ = swd::append_swd_log(&[tx_err.clone()]);
                if let Some(ref sender) = self.tui_sender {
                    let _ = sender.send(tui::TuiMsg::SwdResult(tx_err));
                }
                let hint = format!(
                    "SWD verification failed for {path}:\n\
                     - Before hash: {before}\n\
                     - After hash: {after}\n\
                     - Reason: {detail}\n\
                     The file has been rolled back. Please retry with corrected content.",
                    before = before_hash.as_deref().unwrap_or("none"),
                    after = after_hash.as_deref().unwrap_or("none"),
                );
                return Err(ToolError::new(hint));
            } else {
                let ts2 = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                    .unwrap_or(0);
                let tx_err = SwdTransaction {
                    tool_name: tool_name.to_string(),
                    path: path.clone(),
                    before_hash: before_hash.clone(),
                    after_hash: after_hash.clone(),
                    outcome: outcome.clone(),
                    timestamp_ms: ts2,
                };
                let _ = swd::append_swd_log(&[tx_err.clone()]);
                if let Some(ref sender) = self.tui_sender {
                    let _ = sender.send(tui::TuiMsg::SwdResult(tx_err));
                }
                return Err(ToolError::new(format!(
                    "SWD max retries exceeded for {path}: {detail}. Manual intervention required."
                )));
            }
        }

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        let tx_record = SwdTransaction {
            tool_name: tool_name.to_string(),
            path: path.clone(),
            before_hash,
            after_hash,
            outcome,
            timestamp_ms: ts,
        };
        let _ = swd::append_swd_log(&[tx_record.clone()]);

        if let Some(ref sender) = self.tui_sender {
            let _ = sender.send(tui::TuiMsg::SwdResult(tx_record));
        }

        // Also send normal ToolResult for UI display.
        match &result {
            Ok(_) => {
                if let Some(ref tx_sender) = self.tui_sender {
                    let _ = tx_sender.send(tui::TuiMsg::ToolResult { ok: true });
                }
            }
            Err(_) => {
                if let Some(ref tx_sender) = self.tui_sender {
                    let _ = tx_sender.send(tui::TuiMsg::ToolResult { ok: false });
                }
            }
        }

        result.map_err(ToolError::new)
    }
}

impl ToolExecutor for CliToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if self
            .allowed_tools
            .as_ref()
            .is_some_and(|allowed| !allowed.iter().any(|p| p.matches(tool_name)))
        {
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is not enabled by the current --allowedTools setting"
            )));
        }

        // Rate limit check (rolling window, in-memory per session).
        if let Err(retry_after) = check_rate_limit(tool_name) {
            return Err(ToolError::new(format!(
                "Rate limit exceeded for `{tool_name}`. Retry after {:.1}s",
                retry_after.as_secs_f32()
            )));
        }

        // SWD interception: full mode blocks writes; partial mode wraps them.
        {
            use std::sync::atomic::Ordering;
            let swd_lv = crate::swd::SwdLevel::from_u8(self.swd_level.load(Ordering::Relaxed));
            if swd_lv == crate::swd::SwdLevel::Full
                && SWD_WRITE_TOOLS.contains(&tool_name)
            {
                let msg = format!(
                    "SWD full mode: '{tool_name}' está bloqueada. Use [FILE_ACTION] blocks no texto."
                );
                if let Some(ref tx) = self.tui_sender {
                    let _ = tx.send(tui::TuiMsg::ToolResult { ok: false });
                }
                return Err(ToolError::new(msg));
            }
            if swd_lv == crate::swd::SwdLevel::Partial
                && SWD_WRITE_TOOLS.contains(&tool_name)
            {
                return self.execute_with_swd(tool_name, input);
            }
        }

        let value = serde_json::from_str(input)
            .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
        match self.tool_registry.execute(tool_name, &value) {
            Ok(output) => {
                if let Some(ref tx) = self.tui_sender {
                    let _ = tx.send(tui::TuiMsg::ToolResult { ok: true });
                } else if self.emit_output {
                    if cli_tools_verbose() {
                        // Modo verboso: bloco completo com output formatado.
                        let markdown = format_tool_result(tool_name, &output, false);
                        self.renderer
                            .stream_markdown(&markdown, &mut io::stdout())
                            .map_err(|error| ToolError::new(error.to_string()))?;
                    } else {
                        // Modo compacto: apenas a marcação ✓ indentada, sem
                        // repetir nome ou despejar output.
                        let mut stdout = io::stdout();
                        writeln!(stdout, "{}", format_tool_result_compact(true))
                            .and_then(|()| stdout.flush())
                            .map_err(|error| ToolError::new(error.to_string()))?;
                    }
                }
                Ok(output)
            }
            Err(error) => {
                if let Some(ref tx) = self.tui_sender {
                    let _ = tx.send(tui::TuiMsg::ToolResult { ok: false });
                } else if self.emit_output {
                    if cli_tools_verbose() {
                        let markdown = format_tool_result(tool_name, &error, true);
                        self.renderer
                            .stream_markdown(&markdown, &mut io::stdout())
                            .map_err(|stream_error| ToolError::new(stream_error.to_string()))?;
                    } else {
                        // Erros sempre mostram a mensagem abaixo da marcação ✗,
                        // mesmo no modo compacto — o usuário precisa ver para
                        // depurar.
                        let mut stdout = io::stdout();
                        let trimmed = error.trim();
                        let body = if trimmed.is_empty() {
                            String::new()
                        } else {
                            let truncated: String = trimmed.chars().take(160).collect();
                            format!("\n      \x1b[38;5;203m{truncated}\x1b[0m")
                        };
                        writeln!(stdout, "{}{}", format_tool_result_compact(false), body)
                            .and_then(|()| stdout.flush())
                            .map_err(|stream_error| ToolError::new(stream_error.to_string()))?;
                    }
                }
                Err(ToolError::new(error))
            }
        }
    }
}

fn permission_policy(mode: PermissionMode, tool_registry: &GlobalToolRegistry) -> PermissionPolicy {
    tool_registry.permission_specs(None).into_iter().fold(
        PermissionPolicy::new(mode),
        |policy, (name, required_permission)| {
            policy.with_tool_requirement(name, required_permission)
        },
    )
}

fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };
            let content = message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => InputContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse { id, name, input } => InputContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::from_str(input)
                            .unwrap_or_else(|_| serde_json::json!({ "raw": input })),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => InputContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![ToolResultContentBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    },
                })
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| InputMessage {
                role: role.to_string(),
                content,
            })
        })
        .collect()
}

fn print_help_to(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "elai v{VERSION}")?;
    writeln!(out)?;
    writeln!(out, "Usage:")?;
    writeln!(
        out,
        "  elai [--model MODEL] [--allowedTools TOOL[,TOOL...]]"
    )?;
    writeln!(out, "      Start the interactive REPL")?;
    writeln!(
        out,
        "  elai [--model MODEL] [--output-format text|json] prompt TEXT"
    )?;
    writeln!(out, "      Send one prompt and exit")?;
    writeln!(
        out,
        "  elai [--model MODEL] [--output-format text|json] TEXT"
    )?;
    writeln!(out, "      Shorthand non-interactive prompt mode")?;
    writeln!(
        out,
        "  elai --resume SESSION.json [/status] [/compact] [...]"
    )?;
    writeln!(
        out,
        "      Inspect or maintain a saved session without entering the REPL"
    )?;
    writeln!(out, "  elai dump-manifests")?;
    writeln!(out, "  elai bootstrap-plan")?;
    writeln!(out, "  elai agents")?;
    writeln!(out, "  elai skills")?;
    writeln!(out, "  elai system-prompt [--cwd PATH] [--date YYYY-MM-DD]")?;
    writeln!(out, "  elai login")?;
    writeln!(out, "  elai logout")?;
    writeln!(out, "  elai init")?;
    writeln!(out)?;
    writeln!(out, "Flags:")?;
    writeln!(
        out,
        "  --model MODEL              Override the active model"
    )?;
    writeln!(
        out,
        "  --output-format FORMAT     Non-interactive output format: text or json"
    )?;
    writeln!(
        out,
        "  --permission-mode MODE     Set read-only, workspace-write, or danger-full-access"
    )?;
    writeln!(
        out,
        "  --dangerously-skip-permissions  Skip all permission checks"
    )?;
    writeln!(out, "  --allowedTools TOOLS       Restrict enabled tools (repeatable; comma-separated aliases supported)")?;
    writeln!(
        out,
        "  --version, -V              Print version and build information locally"
    )?;
    writeln!(out)?;
    writeln!(out, "Interactive slash commands:")?;
    writeln!(out, "{}", render_slash_command_help())?;
    writeln!(out)?;
    let resume_commands = resume_supported_slash_commands()
        .into_iter()
        .map(|spec| match spec.argument_hint() {
            Some(argument_hint) => format!("/{} {}", spec.name, argument_hint),
            None => format!("/{}", spec.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "Resume-safe commands: {resume_commands}")?;
    writeln!(out, "Examples:")?;
    writeln!(out, "  elai --model opus \"summarize this repo\"")?;
    writeln!(
        out,
        "  elai --output-format json prompt \"explain src/main.rs\""
    )?;
    writeln!(
        out,
        "  elai --allowedTools read,glob \"summarize Cargo.toml\""
    )?;
    writeln!(
        out,
        "  elai --resume session.json /status /diff /export notes.txt"
    )?;
    writeln!(out, "  elai agents")?;
    writeln!(out, "  elai /skills")?;
    writeln!(out, "  elai login")?;
    writeln!(out, "  elai init")?;
    Ok(())
}

fn print_help() {
    let _ = print_help_to(&mut io::stdout());
}

fn sync_session_to_app_chat(session: &Session, app: &mut tui::UiApp) {
    use std::collections::HashMap;

    app.chat.clear();

    // Pre-build tool_use_id → is_error map from all ToolResult blocks.
    let mut tool_statuses: HashMap<&str, bool> = HashMap::new();
    for msg in &session.messages {
        for block in &msg.blocks {
            if let runtime::ContentBlock::ToolResult { tool_use_id, is_error, .. } = block {
                tool_statuses.insert(tool_use_id.as_str(), *is_error);
            }
        }
    }

    for msg in &session.messages {
        match msg.role {
            runtime::MessageRole::System => {}
            runtime::MessageRole::User => {
                let text: String = msg
                    .blocks
                    .iter()
                    .filter_map(|b| {
                        if let runtime::ContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    app.push_chat(tui::ChatEntry::UserMessage(text));
                }
            }
            runtime::MessageRole::Assistant => {
                let text: String = msg
                    .blocks
                    .iter()
                    .filter_map(|b| {
                        if let runtime::ContentBlock::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    app.push_chat(tui::ChatEntry::AssistantText(text));
                }

                let items: Vec<tui::ToolBatchItem> = msg
                    .blocks
                    .iter()
                    .filter_map(|b| {
                        if let runtime::ContentBlock::ToolUse { id, name, input } = b {
                            let status = match tool_statuses.get(id.as_str()) {
                                Some(true) => tui::ToolItemStatus::Err,
                                _ => tui::ToolItemStatus::Ok,
                            };
                            Some(tui::ToolBatchItem {
                                name: name.clone(),
                                input_summary: tool_input_one_line(name, input),
                                status,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                if !items.is_empty() {
                    app.push_chat(tui::ChatEntry::ToolBatchEntry { items, closed: true });
                }
            }
            runtime::MessageRole::Tool => {}
        }
    }

    app.chat_scroll = usize::MAX;
}

#[cfg(test)]
mod tests {
    use super::{
        describe_tool_progress, filter_tool_specs, format_compact_report, format_cost_report,
        format_internal_prompt_progress_line, format_model_report, format_model_switch_report,
        format_permissions_report, format_permissions_switch_report, format_resume_report,
        format_status_report, format_tool_call_start, format_tool_result,
        normalize_permission_mode, parse_args, parse_git_status_metadata, permission_policy,
        print_help_to, push_output_block, render_config_report, render_memory_report,
        render_repl_help, response_to_events, resume_supported_slash_commands, status_context,
        CliAction, CliOutputFormat, InternalPromptProgressEvent, InternalPromptProgressState,
        SlashCommand, StatusUsage,
    };
    use api::{
        resolve_model_alias, suggested_default_model, MessageResponse, OutputContentBlock, Usage,
    };
    use plugins::{PluginTool, PluginToolDefinition, PluginToolPermission};
    use runtime::{AssistantEvent, ContentBlock, ConversationMessage, MessageRole, PermissionMode};
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::Duration;
    use tools::{GlobalToolRegistry, MatcherPattern};

    fn registry_with_plugin_tool() -> GlobalToolRegistry {
        GlobalToolRegistry::with_plugin_tools(vec![PluginTool::new(
            "plugin-demo@external",
            "plugin-demo",
            PluginToolDefinition {
                name: "plugin_echo".to_string(),
                description: Some("Echo plugin payload".to_string()),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            },
            "echo".to_string(),
            Vec::new(),
            PluginToolPermission::WorkspaceWrite,
            None,
        )])
        .expect("plugin tool registry should build")
    }

    #[test]
    fn defaults_to_repl_when_no_args() {
        assert_eq!(
            parse_args(&[]).expect("args should parse"),
            CliAction::Repl {
                model: suggested_default_model(),
                allowed_tools: None,
                permission_mode: PermissionMode::DangerFullAccess,
                no_tui: false,
                swd_level: crate::swd::SwdLevel::default(),
                budget_config: None,
                no_cache: false,
            }
        );
    }

    #[test]
    fn parses_prompt_subcommand() {
        let args = vec![
            "prompt".to_string(),
            "hello".to_string(),
            "world".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::Prompt {
                prompt: "hello world".to_string(),
                model: suggested_default_model(),
                output_format: CliOutputFormat::Text,
                allowed_tools: None,
                permission_mode: PermissionMode::DangerFullAccess,
            }
        );
    }

    #[test]
    fn parses_bare_prompt_and_json_output_flag() {
        let args = vec![
            "--output-format=json".to_string(),
            "--model".to_string(),
            "custom-opus".to_string(),
            "explain".to_string(),
            "this".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::Prompt {
                prompt: "explain this".to_string(),
                model: "custom-opus".to_string(),
                output_format: CliOutputFormat::Json,
                allowed_tools: None,
                permission_mode: PermissionMode::DangerFullAccess,
            }
        );
    }

    #[test]
    fn resolves_model_aliases_in_args() {
        let args = vec![
            "--model".to_string(),
            "opus".to_string(),
            "explain".to_string(),
            "this".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::Prompt {
                prompt: "explain this".to_string(),
                model: "claude-opus-4-6".to_string(),
                output_format: CliOutputFormat::Text,
                allowed_tools: None,
                permission_mode: PermissionMode::DangerFullAccess,
            }
        );
    }

    #[test]
    fn resolves_known_model_aliases() {
        assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251001");
        assert_eq!(resolve_model_alias("custom-opus"), "custom-opus");
    }

    #[test]
    fn parses_version_flags_without_initializing_prompt_mode() {
        assert_eq!(
            parse_args(&["--version".to_string()]).expect("args should parse"),
            CliAction::Version
        );
        assert_eq!(
            parse_args(&["-V".to_string()]).expect("args should parse"),
            CliAction::Version
        );
    }

    #[test]
    fn parses_permission_mode_flag() {
        let args = vec!["--permission-mode=read-only".to_string()];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::Repl {
                model: suggested_default_model(),
                allowed_tools: None,
                permission_mode: PermissionMode::ReadOnly,
                no_tui: false,
                swd_level: crate::swd::SwdLevel::default(),
                budget_config: None,
                no_cache: false,
            }
        );
    }

    #[test]
    fn parses_allowed_tools_flags_with_aliases_and_lists() {
        let args = vec![
            "--allowedTools".to_string(),
            "read,glob".to_string(),
            "--allowed-tools=write_file".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::Repl {
                model: suggested_default_model(),
                allowed_tools: Some(vec![
                    MatcherPattern::Exact("read_file".to_string()),
                    MatcherPattern::Exact("glob_search".to_string()),
                    MatcherPattern::Exact("write_file".to_string()),
                ]),
                permission_mode: PermissionMode::DangerFullAccess,
                no_tui: false,
                swd_level: crate::swd::SwdLevel::default(),
                budget_config: None,
                no_cache: false,
            }
        );
    }

    #[test]
    fn rejects_unknown_allowed_tools() {
        let error = parse_args(&["--allowedTools".to_string(), "teleport".to_string()])
            .expect_err("tool should be rejected");
        assert!(error.contains("unsupported tool in --allowedTools: teleport"));
    }

    #[test]
    fn parses_system_prompt_options() {
        let args = vec![
            "system-prompt".to_string(),
            "--cwd".to_string(),
            "/tmp/project".to_string(),
            "--date".to_string(),
            "2026-04-01".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::PrintSystemPrompt {
                cwd: PathBuf::from("/tmp/project"),
                date: "2026-04-01".to_string(),
            }
        );
    }

    #[test]
    fn parses_login_and_logout_subcommands() {
        assert_eq!(
            parse_args(&["login".to_string()]).expect("login should parse"),
            CliAction::Login(crate::args::LoginArgs::default())
        );
        assert_eq!(
            parse_args(&["logout".to_string()]).expect("logout should parse"),
            CliAction::Logout
        );
        assert_eq!(
            parse_args(&["init".to_string()]).expect("init should parse"),
            CliAction::Init(crate::args::InitArgs::default())
        );
        assert_eq!(
            parse_args(&["agents".to_string()]).expect("agents should parse"),
            CliAction::Agents { args: None }
        );
        assert_eq!(
            parse_args(&["skills".to_string()]).expect("skills should parse"),
            CliAction::Skills { args: None }
        );
        assert_eq!(
            parse_args(&["agents".to_string(), "--help".to_string()])
                .expect("agents help should parse"),
            CliAction::Agents {
                args: Some("--help".to_string())
            }
        );
    }

    #[test]
    fn parses_direct_agents_and_skills_slash_commands() {
        assert_eq!(
            parse_args(&["/agents".to_string()]).expect("/agents should parse"),
            CliAction::Agents { args: None }
        );
        assert_eq!(
            parse_args(&["/skills".to_string()]).expect("/skills should parse"),
            CliAction::Skills { args: None }
        );
        assert_eq!(
            parse_args(&["/skills".to_string(), "help".to_string()])
                .expect("/skills help should parse"),
            CliAction::Skills {
                args: Some("help".to_string())
            }
        );
        let error = parse_args(&["/status".to_string()])
            .expect_err("/status should remain REPL-only when invoked directly");
        assert!(error.contains("unsupported direct slash command"));
    }

    #[test]
    fn parses_resume_flag_with_slash_command() {
        let args = vec![
            "--resume".to_string(),
            "session.json".to_string(),
            "/compact".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::ResumeSession {
                session_path: PathBuf::from("session.json"),
                commands: vec!["/compact".to_string()],
            }
        );
    }

    #[test]
    fn parses_resume_flag_with_multiple_slash_commands() {
        let args = vec![
            "--resume".to_string(),
            "session.json".to_string(),
            "/status".to_string(),
            "/compact".to_string(),
            "/cost".to_string(),
        ];
        assert_eq!(
            parse_args(&args).expect("args should parse"),
            CliAction::ResumeSession {
                session_path: PathBuf::from("session.json"),
                commands: vec![
                    "/status".to_string(),
                    "/compact".to_string(),
                    "/cost".to_string(),
                ],
            }
        );
    }

    #[test]
    fn filtered_tool_specs_respect_allowlist() {
        let allowed = vec![
            MatcherPattern::Exact("read_file".to_string()),
            MatcherPattern::Exact("grep_search".to_string()),
        ];
        let filtered = filter_tool_specs(&GlobalToolRegistry::builtin(), Some(&allowed), &runtime::ToolCatalog::default(), None);
        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["read_file", "grep_search"]);
    }

    #[test]
    fn filtered_tool_specs_include_plugin_tools() {
        let filtered = filter_tool_specs(&registry_with_plugin_tool(), None, &runtime::ToolCatalog::default(), None);
        let names = filtered
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"plugin_echo".to_string()));
    }

    #[test]
    fn permission_policy_uses_plugin_tool_permissions() {
        let policy = permission_policy(PermissionMode::ReadOnly, &registry_with_plugin_tool());
        let required = policy.required_mode_for("plugin_echo");
        assert_eq!(required, PermissionMode::WorkspaceWrite);
    }

    #[test]
    fn permission_policy_builtin_unchanged() {
        use tools::mvp_tool_specs;
        let registry = GlobalToolRegistry::builtin();
        let policy = permission_policy(PermissionMode::ReadOnly, &registry);
        // Verify every builtin tool has a permission requirement registered in the policy.
        for spec in mvp_tool_specs() {
            let required = policy.required_mode_for(spec.name);
            assert_eq!(
                required, spec.required_permission,
                "permission for builtin tool '{}' changed: got {:?}, want {:?}",
                spec.name, required, spec.required_permission
            );
        }
    }

    #[test]
    fn shared_help_uses_resume_annotation_copy() {
        let help = commands::render_slash_command_help();
        assert!(help.contains("Slash commands"));
        assert!(help.contains("works with --resume SESSION.json"));
    }

    #[test]
    fn repl_help_includes_shared_commands_and_exit() {
        let help = render_repl_help();
        assert!(help.contains("REPL"));
        assert!(help.contains("/help"));
        assert!(help.contains("/status"));
        assert!(help.contains("/model [model]"));
        assert!(help.contains("/permissions [read-only|workspace-write|danger-full-access]"));
        assert!(help.contains("/clear [--confirm]"));
        assert!(help.contains("/cost"));
        assert!(help.contains("/resume <session-path>"));
        assert!(help.contains("/config [env|hooks|model|plugins]"));
        assert!(help.contains("/memory"));
        assert!(help.contains("/init"));
        assert!(help.contains("/diff"));
        assert!(help.contains("/version"));
        assert!(help.contains("/export [file]"));
        assert!(help.contains("/session [list|switch <session-id>]"));
        assert!(help.contains(
            "/plugin [list|install <path>|enable <name>|disable <name>|uninstall <id>|update <id>]"
        ));
        assert!(help.contains("aliases: /plugins, /marketplace"));
        assert!(help.contains("/agents"));
        assert!(help.contains("/skills"));
        assert!(help.contains("/exit"));
    }

    #[test]
    fn resume_supported_command_list_matches_expected_surface() {
        let names = resume_supported_slash_commands()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "help", "status", "compact", "clear", "cost", "config", "memory", "init", "diff",
                "version", "export", "agents", "skills", "budget", "tools", "stats", "providers",
                "verify", "locale",
            ]
        );
    }

    #[test]
    fn resume_report_uses_sectioned_layout() {
        let report = format_resume_report("session.json", 14, 6);
        assert!(report.contains("Session resumed"));
        assert!(report.contains("Session file     session.json"));
        assert!(report.contains("Messages         14"));
        assert!(report.contains("Turns            6"));
    }

    #[test]
    fn compact_report_uses_structured_output() {
        let compacted = format_compact_report(8, 5, false);
        assert!(compacted.contains("Compact"));
        assert!(compacted.contains("Result           compacted"));
        assert!(compacted.contains("Messages removed 8"));
        let skipped = format_compact_report(0, 3, true);
        assert!(skipped.contains("Result           skipped"));
    }

    #[test]
    fn cost_report_uses_sectioned_layout() {
        let report = format_cost_report(runtime::TokenUsage {
            input_tokens: 20,
            output_tokens: 8,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 1,
        });
        assert!(report.contains("Cost"));
        assert!(report.contains("Input tokens     20"));
        assert!(report.contains("Output tokens    8"));
        assert!(report.contains("Cache create     3"));
        assert!(report.contains("Cache read       1"));
        assert!(report.contains("Total tokens     32"));
    }

    #[test]
    fn permissions_report_uses_sectioned_layout() {
        let report = format_permissions_report("workspace-write");
        assert!(report.contains("Permissions"));
        assert!(report.contains("Active mode      workspace-write"));
        assert!(report.contains("Modes"));
        assert!(report.contains("read-only          ○ available Read/search tools only"));
        assert!(report.contains("workspace-write    ● current   Edit files inside the workspace"));
        assert!(report.contains("danger-full-access ○ available Unrestricted tool access"));
    }

    #[test]
    fn permissions_switch_report_is_structured() {
        let report = format_permissions_switch_report("read-only", "workspace-write");
        assert!(report.contains("Permissions updated"));
        assert!(report.contains("Result           mode switched"));
        assert!(report.contains("Previous mode    read-only"));
        assert!(report.contains("Active mode      workspace-write"));
        assert!(report.contains("Applies to       subsequent tool calls"));
    }

    #[test]
    fn init_help_mentions_direct_subcommand() {
        let mut help = Vec::new();
        print_help_to(&mut help).expect("help should render");
        let help = String::from_utf8(help).expect("help should be utf8");
        assert!(help.contains("elai init"));
        assert!(help.contains("elai agents"));
        assert!(help.contains("elai skills"));
        assert!(help.contains("elai /skills"));
    }

    #[test]
    fn model_report_uses_sectioned_layout() {
        let report = format_model_report("sonnet", 12, 4);
        assert!(report.contains("Model"));
        assert!(report.contains("Current model    sonnet"));
        assert!(report.contains("Session messages 12"));
        assert!(report.contains("Switch models with /model <name>"));
    }

    #[test]
    fn model_switch_report_preserves_context_summary() {
        let report = format_model_switch_report("sonnet", "opus", 9);
        assert!(report.contains("Model updated"));
        assert!(report.contains("Previous         sonnet"));
        assert!(report.contains("Current          opus"));
        assert!(report.contains("Preserved msgs   9"));
    }

    #[test]
    fn status_line_reports_model_and_token_totals() {
        let status = format_status_report(
            "sonnet",
            StatusUsage {
                message_count: 7,
                turns: 3,
                latest: runtime::TokenUsage {
                    input_tokens: 5,
                    output_tokens: 4,
                    cache_creation_input_tokens: 1,
                    cache_read_input_tokens: 0,
                },
                cumulative: runtime::TokenUsage {
                    input_tokens: 20,
                    output_tokens: 8,
                    cache_creation_input_tokens: 2,
                    cache_read_input_tokens: 1,
                },
                estimated_tokens: 128,
            },
            "workspace-write",
            &super::StatusContext {
                cwd: PathBuf::from("/tmp/project"),
                session_path: Some(PathBuf::from("session.json")),
                loaded_config_files: 2,
                discovered_config_files: 3,
                memory_file_count: 4,
                project_root: Some(PathBuf::from("/tmp")),
                git_branch: Some("main".to_string()),
            },
        );
        assert!(status.contains("Status"));
        assert!(status.contains("Model            sonnet"));
        assert!(status.contains("Permission mode  workspace-write"));
        assert!(status.contains("Messages         7"));
        assert!(status.contains("Latest total     10"));
        assert!(status.contains("Cumulative total 31"));
        assert!(status.contains("Cwd              /tmp/project"));
        assert!(status.contains("Project root     /tmp"));
        assert!(status.contains("Git branch       main"));
        assert!(status.contains("Session          session.json"));
        assert!(status.contains("Config files     loaded 2/3"));
        assert!(status.contains("Memory files     4"));
    }

    #[test]
    fn config_report_supports_section_views() {
        let report = render_config_report(Some("env")).expect("config report should render");
        assert!(report.contains("Merged section: env"));
        let plugins_report =
            render_config_report(Some("plugins")).expect("plugins config report should render");
        assert!(plugins_report.contains("Merged section: plugins"));
    }

    #[test]
    fn memory_report_uses_sectioned_layout() {
        let report = render_memory_report().expect("memory report should render");
        assert!(report.contains("Memory"));
        assert!(report.contains("Working directory"));
        assert!(report.contains("Instruction files"));
        assert!(report.contains("Discovered files"));
    }

    #[test]
    fn config_report_uses_sectioned_layout() {
        let report = render_config_report(None).expect("config report should render");
        assert!(report.contains("Config"));
        assert!(report.contains("Discovered files"));
        assert!(report.contains("Merged JSON"));
    }

    #[test]
    fn parses_git_status_metadata() {
        let (root, branch) = parse_git_status_metadata(Some(
            "## rcc/cli...origin/rcc/cli
 M src/main.rs",
        ));
        assert_eq!(branch.as_deref(), Some("rcc/cli"));
        let _ = root;
    }

    #[test]
    fn status_context_reads_real_workspace_metadata() {
        let context = status_context(None).expect("status context should load");
        assert!(context.cwd.is_absolute());
        assert_eq!(context.discovered_config_files, 5);
        assert!(context.loaded_config_files <= context.discovered_config_files);
    }

    #[test]
    fn normalizes_supported_permission_modes() {
        assert_eq!(normalize_permission_mode("read-only"), Some("read-only"));
        assert_eq!(
            normalize_permission_mode("workspace-write"),
            Some("workspace-write")
        );
        assert_eq!(
            normalize_permission_mode("danger-full-access"),
            Some("danger-full-access")
        );
        assert_eq!(normalize_permission_mode("unknown"), None);
    }

    #[test]
    fn clear_command_requires_explicit_confirmation_flag() {
        assert_eq!(
            SlashCommand::parse("/clear"),
            Some(SlashCommand::Clear { confirm: false })
        );
        assert_eq!(
            SlashCommand::parse("/clear --confirm"),
            Some(SlashCommand::Clear { confirm: true })
        );
    }

    #[test]
    fn parses_resume_and_config_slash_commands() {
        assert_eq!(
            SlashCommand::parse("/resume saved-session.json"),
            Some(SlashCommand::Resume {
                session_path: Some("saved-session.json".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/clear --confirm"),
            Some(SlashCommand::Clear { confirm: true })
        );
        assert_eq!(
            SlashCommand::parse("/config"),
            Some(SlashCommand::Config { section: None })
        );
        assert_eq!(
            SlashCommand::parse("/config env"),
            Some(SlashCommand::Config {
                section: Some("env".to_string())
            })
        );
        assert_eq!(SlashCommand::parse("/memory"), Some(SlashCommand::Memory));
        assert_eq!(SlashCommand::parse("/init"), Some(SlashCommand::Init));
    }

    #[test]
    fn init_template_mentions_detected_rust_workspace() {
        // O novo render_init_elai_md usa collect_facts → render_static_elai_md (grounded
        // em fatos extraídos), substituindo o template estático antigo. Verifica apenas
        // que o cabeçalho e seções principais foram renderizados.
        let rendered = crate::init::render_init_elai_md(std::path::Path::new("."));
        assert!(rendered.contains("# ELAI.md"));
        assert!(rendered.contains("## Estrutura"));
    }

    #[test]
    fn converts_tool_roundtrip_messages() {
        let messages = vec![
            ConversationMessage::user_text("hello"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: "{\"command\":\"pwd\"}".to_string(),
            }]),
            ConversationMessage {
                role: MessageRole::Tool,
                blocks: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    tool_name: "bash".to_string(),
                    output: "ok".to_string(),
                    is_error: false,
                }],
                usage: None,
            },
        ];

        let converted = super::convert_messages(&messages);
        assert_eq!(converted.len(), 3);
        assert_eq!(converted[1].role, "assistant");
        assert_eq!(converted[2].role, "user");
    }
    #[test]
    fn repl_help_mentions_history_completion_and_multiline() {
        let help = render_repl_help();
        assert!(help.contains("Up/Down"));
        assert!(help.contains("Tab"));
        assert!(help.contains("Shift+Enter/Ctrl+J"));
    }

    #[test]
    fn tool_rendering_helpers_compact_output() {
        let start = format_tool_call_start("read_file", r#"{"path":"src/main.rs"}"#);
        assert!(start.contains("read_file"));
        assert!(start.contains("src/main.rs"));

        let done = format_tool_result(
            "read_file",
            r#"{"file":{"filePath":"src/main.rs","content":"hello","numLines":1,"startLine":1,"totalLines":1}}"#,
            false,
        );
        assert!(done.contains("📄 Read src/main.rs"));
        assert!(done.contains("hello"));
    }

    #[test]
    fn tool_rendering_truncates_large_read_output_for_display_only() {
        let content = (0..200)
            .map(|index| format!("line {index:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = json!({
            "file": {
                "filePath": "src/main.rs",
                "content": content,
                "numLines": 200,
                "startLine": 1,
                "totalLines": 200
            }
        })
        .to_string();

        let rendered = format_tool_result("read_file", &output, false);

        assert!(rendered.contains("line 000"));
        assert!(rendered.contains("line 079"));
        assert!(!rendered.contains("line 199"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("line 199"));
    }

    #[test]
    fn tool_rendering_truncates_large_bash_output_for_display_only() {
        let stdout = (0..120)
            .map(|index| format!("stdout {index:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = json!({
            "stdout": stdout,
            "stderr": "",
            "returnCodeInterpretation": "completed successfully"
        })
        .to_string();

        let rendered = format_tool_result("bash", &output, false);

        assert!(rendered.contains("stdout 000"));
        assert!(rendered.contains("stdout 059"));
        assert!(!rendered.contains("stdout 119"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("stdout 119"));
    }

    #[test]
    fn tool_rendering_truncates_generic_long_output_for_display_only() {
        let items = (0..120)
            .map(|index| format!("payload {index:03}"))
            .collect::<Vec<_>>();
        let output = json!({
            "summary": "plugin payload",
            "items": items,
        })
        .to_string();

        let rendered = format_tool_result("plugin_echo", &output, false);

        assert!(rendered.contains("plugin_echo"));
        assert!(rendered.contains("payload 000"));
        assert!(rendered.contains("payload 040"));
        assert!(!rendered.contains("payload 080"));
        assert!(!rendered.contains("payload 119"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("payload 119"));
    }

    #[test]
    fn tool_rendering_truncates_raw_generic_output_for_display_only() {
        let output = (0..120)
            .map(|index| format!("raw {index:03}"))
            .collect::<Vec<_>>()
            .join("\n");

        let rendered = format_tool_result("plugin_echo", &output, false);

        assert!(rendered.contains("plugin_echo"));
        assert!(rendered.contains("raw 000"));
        assert!(rendered.contains("raw 059"));
        assert!(!rendered.contains("raw 119"));
        assert!(rendered.contains("full result preserved in session"));
        assert!(output.contains("raw 119"));
    }

    #[test]
    fn ultraplan_progress_lines_include_phase_step_and_elapsed_status() {
        let snapshot = InternalPromptProgressState {
            command_label: "Ultraplan",
            task_label: "ship plugin progress".to_string(),
            step: 3,
            phase: "running read_file".to_string(),
            detail: Some("reading rust/crates/elai-cli/src/main.rs".to_string()),
            saw_final_text: false,
        };

        let started = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Started,
            &snapshot,
            Duration::from_secs(0),
            None,
        );
        let heartbeat = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Heartbeat,
            &snapshot,
            Duration::from_secs(9),
            None,
        );
        let completed = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Complete,
            &snapshot,
            Duration::from_secs(12),
            None,
        );
        let failed = format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Failed,
            &snapshot,
            Duration::from_secs(12),
            Some("network timeout"),
        );

        assert!(started.contains("planning started"));
        assert!(started.contains("current step 3"));
        assert!(heartbeat.contains("heartbeat"));
        assert!(heartbeat.contains("9s elapsed"));
        assert!(heartbeat.contains("phase running read_file"));
        assert!(completed.contains("completed"));
        assert!(completed.contains("3 steps total"));
        assert!(failed.contains("failed"));
        assert!(failed.contains("network timeout"));
    }

    #[test]
    fn describe_tool_progress_summarizes_known_tools() {
        assert_eq!(
            describe_tool_progress("read_file", r#"{"path":"src/main.rs"}"#),
            "reading src/main.rs"
        );
        assert!(
            describe_tool_progress("bash", r#"{"command":"cargo test -p elai-cli"}"#)
                .contains("cargo test -p elai-cli")
        );
        assert_eq!(
            describe_tool_progress("grep_search", r#"{"pattern":"ultraplan","path":"rust"}"#),
            "grep `ultraplan` in rust"
        );
    }

    #[test]
    fn push_output_block_renders_markdown_text() {
        let mut out = Vec::new();
        let mut events = Vec::new();
        let mut pending_tool = None;

        push_output_block(
            OutputContentBlock::Text {
                text: "# Heading".to_string(),
            },
            &mut out,
            &mut events,
            &mut pending_tool,
            false,
        )
        .expect("text block should render");

        let rendered = String::from_utf8(out).expect("utf8");
        assert!(rendered.contains("Heading"));
        assert!(rendered.contains('\u{1b}'));
    }

    #[test]
    fn push_output_block_skips_empty_object_prefix_for_tool_streams() {
        let mut out = Vec::new();
        let mut events = Vec::new();
        let mut pending_tool = None;

        push_output_block(
            OutputContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "read_file".to_string(),
                input: json!({}),
            },
            &mut out,
            &mut events,
            &mut pending_tool,
            true,
        )
        .expect("tool block should accumulate");

        assert!(events.is_empty());
        assert_eq!(
            pending_tool,
            Some(("tool-1".to_string(), "read_file".to_string(), String::new(),))
        );
    }

    #[test]
    fn response_to_events_preserves_empty_object_json_input_outside_streaming() {
        let mut out = Vec::new();
        let events = response_to_events(
            MessageResponse {
                id: "msg-1".to_string(),
                kind: "message".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![OutputContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({}),
                }],
                stop_reason: Some("tool_use".to_string()),
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
                request_id: None,
            },
            &mut out,
        )
        .expect("response conversion should succeed");

        assert!(matches!(
            &events[0],
            AssistantEvent::ToolUse { name, input, .. }
                if name == "read_file" && input == "{}"
        ));
    }

    #[test]
    fn response_to_events_preserves_non_empty_json_input_outside_streaming() {
        let mut out = Vec::new();
        let events = response_to_events(
            MessageResponse {
                id: "msg-2".to_string(),
                kind: "message".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![OutputContentBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "read_file".to_string(),
                    input: json!({ "path": "rust/Cargo.toml" }),
                }],
                stop_reason: Some("tool_use".to_string()),
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
                request_id: None,
            },
            &mut out,
        )
        .expect("response conversion should succeed");

        assert!(matches!(
            &events[0],
            AssistantEvent::ToolUse { name, input, .. }
                if name == "read_file" && input == "{\"path\":\"rust/Cargo.toml\"}"
        ));
    }

    #[test]
    fn response_to_events_ignores_thinking_blocks() {
        let mut out = Vec::new();
        let events = response_to_events(
            MessageResponse {
                id: "msg-3".to_string(),
                kind: "message".to_string(),
                model: "claude-opus-4-6".to_string(),
                role: "assistant".to_string(),
                content: vec![
                    OutputContentBlock::Thinking {
                        thinking: "step 1".to_string(),
                        signature: Some("sig_123".to_string()),
                    },
                    OutputContentBlock::Text {
                        text: "Final answer".to_string(),
                    },
                ],
                stop_reason: Some("end_turn".to_string()),
                stop_sequence: None,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
                request_id: None,
            },
            &mut out,
        )
        .expect("response conversion should succeed");

        assert!(matches!(
            &events[0],
            AssistantEvent::TextDelta(text) if text == "Final answer"
        ));
        assert!(!String::from_utf8(out).expect("utf8").contains("step 1"));
    }
}
