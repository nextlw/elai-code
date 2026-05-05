pub mod providers;
pub mod stats;
pub mod user_commands;
pub use user_commands::{
    expand_template, parse_user_command, UserCommand, UserCommandRegistry, UserCommandScope,
};

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use plugins::{PluginError, PluginManager, PluginSummary};
use runtime::{compact_session, CompactionConfig, Session, ProgressReporter};

// Carrega os arquivos de locale do workspace (`rust/locales/{en,pt-BR}.json`).
// Fonte única do catálogo i18n: outros crates só fazem `rust_i18n::set_locale()`.
// Fallback automático para `en` quando uma chave não existe no idioma ativo.
rust_i18n::i18n!("../../../locales", fallback = "pt-BR");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandManifestEntry {
    pub name: String,
    pub source: CommandSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    InternalOnly,
    FeatureGated,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandRegistry {
    entries: Vec<CommandManifestEntry>,
}

impl CommandRegistry {
    #[must_use]
    pub fn new(entries: Vec<CommandManifestEntry>) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &[CommandManifestEntry] {
        &self.entries
    }
}

/// Categoria de agrupamento exibida no `/help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlashCategory {
    /// /help, /status, /clear, /compact, /resume, /export, /cost
    Session,
    /// /model, /permissions, /tools, /budget, /cache, /providers
    Behavior,
    /// /init, /memory, /config, /verify
    Project,
    /// /diff, /branch, /worktree, /commit, /commit-push-pr, /pr, /issue
    Git,
    /// /bughunter, /ultraplan, /teleport, /debug-tool-call
    Analysis,
    /// /version, /update, /commands
    System,
    /// /plugin, /agents, /skills, /dream, /stats, /session
    Plugins,
    /// comandos .md do usuário
    Custom,
}

impl SlashCategory {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Behavior => "Behavior",
            Self::Project => "Project",
            Self::Git => "Git",
            Self::Analysis => "Analysis",
            Self::System => "System",
            Self::Plugins => "Plugins",
            Self::Custom => "Custom",
        }
    }

    #[must_use]
    pub const fn order(self) -> u8 {
        match self {
            Self::Session => 0,
            Self::Behavior => 1,
            Self::Project => 2,
            Self::Git => 3,
            Self::Analysis => 4,
            Self::System => 5,
            Self::Plugins => 6,
            Self::Custom => 7,
        }
    }
}

/// Predicado padrão: sempre habilitado.
#[must_use]
pub const fn always_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Chave i18n da descrição do comando (ex.: `"commands.help.summary"`).
    /// Use [`SlashCommandSpec::summary`] para resolver no locale ativo.
    pub summary_key: &'static str,
    /// Chave i18n da dica de argumento (ex.: `"commands.model.argument_hint"`).
    /// Use [`SlashCommandSpec::argument_hint`] para resolver no locale ativo.
    pub argument_hint_key: Option<&'static str>,
    pub resume_supported: bool,
    /// Categoria para agrupamento no /help.
    pub category: SlashCategory,
    /// Predicado runtime — se retornar `false` o comando é omitido do /help.
    pub is_enabled: fn() -> bool,
    /// Se `true`, não aparece no /help mas continua parseável.
    pub hidden: bool,
    /// Nome de exibição alternativo; se `None`, usa `name`.
    pub user_facing_name: Option<&'static str>,
}

impl SlashCommandSpec {
    /// Resolve a descrição do comando no locale ativo via `rust-i18n`.
    /// Fallback automático para `en` se a chave não existir no locale atual.
    #[must_use]
    pub fn summary(&self) -> String {
        rust_i18n::t!(self.summary_key).to_string()
    }

    /// Resolve a dica de argumento no locale ativo, se existir.
    #[must_use]
    pub fn argument_hint(&self) -> Option<String> {
        self.argument_hint_key
            .map(|key| rust_i18n::t!(key).to_string())
    }
}

impl PartialEq for SlashCommandSpec {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.aliases == other.aliases
            && self.summary_key == other.summary_key
            && self.argument_hint_key == other.argument_hint_key
            && self.resume_supported == other.resume_supported
            && self.category == other.category
            && self.hidden == other.hidden
            && self.user_facing_name == other.user_facing_name
        // is_enabled intentionally excluded from equality (fn pointer comparison is unreliable)
    }
}

impl Eq for SlashCommandSpec {}

const SLASH_COMMAND_SPECS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        name: "help",
        aliases: &[],
        summary_key: "commands.help.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Session,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "status",
        aliases: &[],
        summary_key: "commands.status.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Session,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "compact",
        aliases: &[],
        summary_key: "commands.compact.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Session,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "model",
        aliases: &[],
        summary_key: "commands.model.summary",
        argument_hint_key: Some("commands.model.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Behavior,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "permissions",
        aliases: &[],
        summary_key: "commands.permissions.summary",
        argument_hint_key: Some("commands.permissions.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Behavior,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "clear",
        aliases: &[],
        summary_key: "commands.clear.summary",
        argument_hint_key: Some("commands.clear.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Session,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "cost",
        aliases: &[],
        summary_key: "commands.cost.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Session,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "resume",
        aliases: &[],
        summary_key: "commands.resume.summary",
        argument_hint_key: Some("commands.resume.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Session,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "config",
        aliases: &[],
        summary_key: "commands.config.summary",
        argument_hint_key: Some("commands.config.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Project,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "memory",
        aliases: &[],
        summary_key: "commands.memory.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Project,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "init",
        aliases: &[],
        summary_key: "commands.init.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Project,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "diff",
        aliases: &[],
        summary_key: "commands.diff.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Git,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "version",
        aliases: &[],
        summary_key: "commands.version.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::System,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "update",
        aliases: &[],
        summary_key: "commands.update.summary",
        argument_hint_key: None,
        resume_supported: false,
        category: SlashCategory::System,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "bughunter",
        aliases: &[],
        summary_key: "commands.bughunter.summary",
        argument_hint_key: Some("commands.bughunter.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Analysis,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "branch",
        aliases: &[],
        summary_key: "commands.branch.summary",
        argument_hint_key: Some("commands.branch.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Git,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "worktree",
        aliases: &[],
        summary_key: "commands.worktree.summary",
        argument_hint_key: Some("commands.worktree.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Git,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "commit",
        aliases: &[],
        summary_key: "commands.commit.summary",
        argument_hint_key: None,
        resume_supported: false,
        category: SlashCategory::Git,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "commit-push-pr",
        aliases: &[],
        summary_key: "commands.commit-push-pr.summary",
        argument_hint_key: Some("commands.commit-push-pr.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Git,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "pr",
        aliases: &[],
        summary_key: "commands.pr.summary",
        argument_hint_key: Some("commands.pr.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Git,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "issue",
        aliases: &[],
        summary_key: "commands.issue.summary",
        argument_hint_key: Some("commands.issue.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Git,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "ultraplan",
        aliases: &[],
        summary_key: "commands.ultraplan.summary",
        argument_hint_key: Some("commands.ultraplan.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Analysis,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "teleport",
        aliases: &[],
        summary_key: "commands.teleport.summary",
        argument_hint_key: Some("commands.teleport.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Analysis,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "debug-tool-call",
        aliases: &[],
        summary_key: "commands.debug-tool-call.summary",
        argument_hint_key: None,
        resume_supported: false,
        category: SlashCategory::Analysis,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "export",
        aliases: &[],
        summary_key: "commands.export.summary",
        argument_hint_key: Some("commands.export.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Session,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "session",
        aliases: &[],
        summary_key: "commands.session.summary",
        argument_hint_key: Some("commands.session.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Plugins,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "plugin",
        aliases: &["plugins", "marketplace"],
        summary_key: "commands.plugin.summary",
        argument_hint_key: Some("commands.plugin.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Plugins,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "agents",
        aliases: &[],
        summary_key: "commands.agents.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Plugins,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "skills",
        aliases: &[],
        summary_key: "commands.skills.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Plugins,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "budget",
        aliases: &[],
        summary_key: "commands.budget.summary",
        argument_hint_key: Some("commands.budget.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Behavior,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "tools",
        aliases: &[],
        summary_key: "commands.tools.summary",
        argument_hint_key: Some("commands.tools.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Behavior,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "cache",
        aliases: &[],
        summary_key: "commands.cache.summary",
        argument_hint_key: Some("commands.cache.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Behavior,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "dream",
        aliases: &[],
        summary_key: "commands.dream.summary",
        argument_hint_key: Some("commands.dream.argument_hint"),
        resume_supported: false,
        category: SlashCategory::Plugins,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "stats",
        aliases: &[],
        summary_key: "commands.stats.summary",
        argument_hint_key: Some("commands.stats.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Plugins,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "providers",
        aliases: &[],
        summary_key: "commands.providers.summary",
        argument_hint_key: Some("commands.providers.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Behavior,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "verify",
        aliases: &[],
        summary_key: "commands.verify.summary",
        argument_hint_key: None,
        resume_supported: true,
        category: SlashCategory::Project,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "locale",
        aliases: &[],
        summary_key: "commands.locale.summary",
        argument_hint_key: Some("commands.locale.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Behavior,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
    SlashCommandSpec {
        name: "deepresearch",
        aliases: &["dr"],
        summary_key: "commands.deepresearch.summary",
        argument_hint_key: Some("commands.deepresearch.argument_hint"),
        resume_supported: true,
        category: SlashCategory::Project,
        is_enabled: always_enabled,
        hidden: false,
        user_facing_name: None,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Status,
    Compact,
    Branch {
        action: Option<String>,
        target: Option<String>,
    },
    Bughunter {
        scope: Option<String>,
    },
    Worktree {
        action: Option<String>,
        path: Option<String>,
        branch: Option<String>,
    },
    Commit,
    CommitPushPr {
        context: Option<String>,
    },
    Pr {
        context: Option<String>,
    },
    Issue {
        context: Option<String>,
    },
    Ultraplan {
        task: Option<String>,
    },
    Teleport {
        target: Option<String>,
    },
    DebugToolCall,
    Model {
        model: Option<String>,
    },
    Permissions {
        mode: Option<String>,
    },
    Clear {
        confirm: bool,
    },
    Cost,
    Resume {
        session_path: Option<String>,
    },
    Config {
        section: Option<String>,
    },
    Memory,
    Init,
    Diff,
    Version,
    Update,
    Export {
        path: Option<String>,
    },
    Session {
        action: Option<String>,
        target: Option<String>,
    },
    Plugins {
        action: Option<String>,
        target: Option<String>,
    },
    Agents {
        args: Option<String>,
    },
    Skills {
        args: Option<String>,
    },
    Budget {
        args: Option<String>,
    },
    Tools {
        subcommand: Option<String>,
    },
    Cache {
        subcommand: Option<String>,
    },
    Dream {
        force: bool,
    },
    Stats {
        days: Option<u32>,
    },
    Providers {
        verbose: bool,
    },
    Verify,
    Locale {
        lang: Option<String>,
    },
    Unknown(String),
}

impl SlashCommand {
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let mut parts = trimmed.trim_start_matches('/').split_whitespace();
        let command = parts.next().unwrap_or_default();
        Some(match command {
            "help" => Self::Help,
            "status" => Self::Status,
            "compact" => Self::Compact,
            "branch" => Self::Branch {
                action: parts.next().map(ToOwned::to_owned),
                target: parts.next().map(ToOwned::to_owned),
            },
            "bughunter" => Self::Bughunter {
                scope: remainder_after_command(trimmed, command),
            },
            "worktree" => Self::Worktree {
                action: parts.next().map(ToOwned::to_owned),
                path: parts.next().map(ToOwned::to_owned),
                branch: parts.next().map(ToOwned::to_owned),
            },
            "commit" => Self::Commit,
            "commit-push-pr" => Self::CommitPushPr {
                context: remainder_after_command(trimmed, command),
            },
            "pr" => Self::Pr {
                context: remainder_after_command(trimmed, command),
            },
            "issue" => Self::Issue {
                context: remainder_after_command(trimmed, command),
            },
            "ultraplan" => Self::Ultraplan {
                task: remainder_after_command(trimmed, command),
            },
            "teleport" => Self::Teleport {
                target: remainder_after_command(trimmed, command),
            },
            "debug-tool-call" => Self::DebugToolCall,
            "model" => Self::Model {
                model: parts.next().map(ToOwned::to_owned),
            },
            "permissions" => Self::Permissions {
                mode: parts.next().map(ToOwned::to_owned),
            },
            "clear" => Self::Clear {
                confirm: parts.next() == Some("--confirm"),
            },
            "cost" => Self::Cost,
            "resume" => Self::Resume {
                session_path: parts.next().map(ToOwned::to_owned),
            },
            "config" => Self::Config {
                section: parts.next().map(ToOwned::to_owned),
            },
            "memory" => Self::Memory,
            "init" => Self::Init,
            "diff" => Self::Diff,
            "version" => Self::Version,
            "update" => Self::Update,
            "export" => Self::Export {
                path: parts.next().map(ToOwned::to_owned),
            },
            "session" => Self::Session {
                action: parts.next().map(ToOwned::to_owned),
                target: parts.next().map(ToOwned::to_owned),
            },
            "plugin" | "plugins" | "marketplace" => Self::Plugins {
                action: parts.next().map(ToOwned::to_owned),
                target: {
                    let remainder = parts.collect::<Vec<_>>().join(" ");
                    (!remainder.is_empty()).then_some(remainder)
                },
            },
            "agents" => Self::Agents {
                args: remainder_after_command(trimmed, command),
            },
            "skills" => Self::Skills {
                args: remainder_after_command(trimmed, command),
            },
            "budget" => Self::Budget {
                args: remainder_after_command(trimmed, command),
            },
            "tools" => Self::Tools {
                subcommand: parts.next().map(ToOwned::to_owned),
            },
            "cache" => Self::Cache {
                subcommand: parts.next().map(ToOwned::to_owned),
            },
            "dream" => Self::Dream {
                force: parts.next() == Some("--force"),
            },
            "stats" => {
                let mut days: Option<u32> = None;
                let remainder = parts.collect::<Vec<_>>().join(" ");
                let mut iter = remainder.split_whitespace();
                while let Some(tok) = iter.next() {
                    if tok == "--days" {
                        days = iter.next().and_then(|v| v.parse().ok());
                    } else if let Some(s) = tok.strip_prefix("--days=") {
                        days = s.parse().ok();
                    }
                }
                Self::Stats { days }
            }
            "providers" => Self::Providers {
                verbose: trimmed.contains("--verbose"),
            },
            "verify" => Self::Verify,
            "locale" => Self::Locale {
                lang: parts.next().map(ToOwned::to_owned),
            },
            "run" => {
                let script = parts.next().map(ToOwned::to_owned).unwrap_or_default();
                let mut update = false;
                let mut args: Option<String> = None;
                let remainder: String = parts.collect::<Vec<_>>().join(" ");
                let mut iter = remainder.split_whitespace();
                while let Some(tok) = iter.next() {
                    match tok {
                        "--update" | "-u" => update = true,
                        "--" => {
                            let rest: String = iter.collect::<Vec<_>>().join(" ");
                            if !rest.is_empty() {
                                args = Some(rest);
                            }
                            break;
                        }
                        other => {
                            if args.is_none() {
                                args = Some(other.to_string());
                            }
                        }
                    }
                }
                Self::Run {
                    script,
                    args,
                    update,
                }
            }
            other => Self::Unknown(other.to_string()),
        })
    }
}

fn remainder_after_command(input: &str, command: &str) -> Option<String> {
    input
        .trim()
        .strip_prefix(&format!("/{command}"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[must_use]
pub fn slash_command_specs() -> &'static [SlashCommandSpec] {
    SLASH_COMMAND_SPECS
}

#[must_use]
pub fn resume_supported_slash_commands() -> Vec<&'static SlashCommandSpec> {
    slash_command_specs()
        .iter()
        .filter(|spec| spec.resume_supported)
        .collect()
}

#[must_use]
pub fn render_slash_command_help() -> String {
    let mut lines = vec![
        "Slash commands".to_string(),
        "  [resume] means the command also works with --resume SESSION.json".to_string(),
    ];
    for spec in slash_command_specs() {
        let name = match spec.argument_hint() {
            Some(argument_hint) => format!("/{} {}", spec.name, argument_hint),
            None => format!("/{}", spec.name),
        };
        let alias_suffix = if spec.aliases.is_empty() {
            String::new()
        } else {
            format!(
                " (aliases: {})",
                spec.aliases
                    .iter()
                    .map(|alias| format!("/{alias}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let resume = if spec.resume_supported {
            " [resume]"
        } else {
            ""
        };
        lines.push(format!(
            "  {name:<20} {}{alias_suffix}{resume}",
            spec.summary()
        ));
    }
    lines.join("\n")
}

/// Renderiza o help agrupado por categoria, respeitando `hidden` e `is_enabled`.
#[must_use]
pub fn render_help_grouped() -> String {
    render_help_grouped_with(None)
}

/// Renderiza o help agrupado, opcionalmente incluindo custom commands do registry.
#[must_use]
pub fn render_help_grouped_with(
    user_commands: Option<&user_commands::UserCommandRegistry>,
) -> String {
    let mut by_cat: std::collections::BTreeMap<u8, Vec<&SlashCommandSpec>> =
        std::collections::BTreeMap::new();
    for spec in SLASH_COMMAND_SPECS {
        if spec.hidden || !(spec.is_enabled)() {
            continue;
        }
        by_cat.entry(spec.category.order()).or_default().push(spec);
    }
    let mut out = String::from("Slash commands\n  [resume] means the command also works with --resume SESSION.json\n");
    for specs in by_cat.values() {
        if specs.is_empty() {
            continue;
        }
        let cat = specs[0].category;
        let _ = writeln!(out, "\n{}", cat.label());
        for spec in specs {
            let display = spec.user_facing_name.unwrap_or(spec.name);
            let resume = if spec.resume_supported { " [resume]" } else { "" };
            let _ = writeln!(out, "  /{display:<18} {}{resume}", spec.summary());
        }
    }
    if let Some(reg) = user_commands {
        if reg.count() > 0 {
            let _ = writeln!(out, "\n{}", SlashCategory::Custom.label());
            let mut customs: Vec<&user_commands::UserCommand> = reg.all().collect();
            customs.sort_by(|a, b| a.name.cmp(&b.name));
            for cmd in customs {
                let scope_marker = match cmd.scope {
                    user_commands::UserCommandScope::Project => "P",
                    user_commands::UserCommandScope::Global => "G",
                };
                let display = if let Some(hint) = &cmd.argument_hint {
                    format!("/{} {}", cmd.name, hint)
                } else {
                    format!("/{}", cmd.name)
                };
                let _ = writeln!(
                    out,
                    "  [{scope_marker}] {display:<18} {}",
                    cmd.description
                );
            }
        }
    }
    out
}

/// Resultado da expansão de um user command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedUserCommand {
    pub command_name: String,
    pub expanded_prompt: String,
    pub argument_hint: Option<String>,
}

/// Tenta parsear e despachar o input contra o registry de user commands.
/// Se houver match, retorna `Some(ExpandedUserCommand)`.
/// O caller (REPL) injeta o registry e usa o template expandido como prompt.
#[must_use]
pub fn try_user_command(
    input: &str,
    registry: &user_commands::UserCommandRegistry,
    cwd: &Path,
) -> Option<ExpandedUserCommand> {
    let trimmed = input.trim().strip_prefix('/').unwrap_or(input.trim());
    let (name, args) = trimmed
        .split_once(' ')
        .map_or((trimmed, ""), |(n, a)| (n, a.trim()));
    let cmd = registry.get(name)?;
    let expanded = user_commands::expand_template(&cmd.body_template, args, cwd);
    Some(ExpandedUserCommand {
        command_name: cmd.name.clone(),
        expanded_prompt: expanded,
        argument_hint: cmd.argument_hint.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandResult {
    pub message: String,
    pub session: Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsCommandResult {
    pub message: String,
    pub reload_runtime: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DefinitionSource {
    ProjectCodex,
    ProjectElai,
    UserCodexHome,
    UserCodex,
    UserElai,
}

impl DefinitionSource {
    fn label(self) -> &'static str {
        match self {
            Self::ProjectCodex => "Project (.codex)",
            Self::ProjectElai => "Project (.elai)",
            Self::UserCodexHome => "User ($CODEX_HOME)",
            Self::UserCodex => "User (~/.codex)",
            Self::UserElai => "User (~/.elai)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentSummary {
    name: String,
    description: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    source: DefinitionSource,
    shadowed_by: Option<DefinitionSource>,
}

#[derive(Debug, Clone)]
struct SkillSummary {
    name: String,
    description: Option<String>,
    source: DefinitionSource,
    shadowed_by: Option<DefinitionSource>,
    origin: SkillOrigin,
    priority: Option<i32>,
    budget_multiplier: Option<f32>,
    force_provider: Option<String>,
    incompatible_with: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillOrigin {
    SkillsDir,
    LegacyCommandsDir,
}

impl SkillOrigin {
    fn detail_label(self) -> Option<&'static str> {
        match self {
            Self::SkillsDir => None,
            Self::LegacyCommandsDir => Some("legacy /commands"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillRoot {
    source: DefinitionSource,
    path: PathBuf,
    origin: SkillOrigin,
}

#[allow(clippy::too_many_lines)]
pub fn handle_plugins_slash_command(
    action: Option<&str>,
    target: Option<&str>,
    manager: &mut PluginManager,
    reporter: &dyn runtime::ProgressReporter,
) -> Result<PluginsCommandResult, PluginError> {
    match action {
        None | Some("list") => Ok(PluginsCommandResult {
            message: render_plugins_report(&manager.list_installed_plugins()?),
            reload_runtime: false,
        }),
        Some("install") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins install <path>".to_string(),
                    reload_runtime: false,
                });
            };
            let install = manager.install(target, reporter)?;
            let plugin = manager
                .list_installed_plugins()?
                .into_iter()
                .find(|plugin| plugin.metadata.id == install.plugin_id);
            Ok(PluginsCommandResult {
                message: render_plugin_install_report(&install.plugin_id, plugin.as_ref()),
                reload_runtime: true,
            })
        }
        Some("enable") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins enable <name>".to_string(),
                    reload_runtime: false,
                });
            };
            let plugin = resolve_plugin_target(manager, target)?;
            manager.enable(&plugin.metadata.id)?;
            Ok(PluginsCommandResult {
                message: format!(
                    "Plugins\n  Result           enabled {}\n  Name             {}\n  Version          {}\n  Status           enabled",
                    plugin.metadata.id, plugin.metadata.name, plugin.metadata.version
                ),
                reload_runtime: true,
            })
        }
        Some("disable") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins disable <name>".to_string(),
                    reload_runtime: false,
                });
            };
            let plugin = resolve_plugin_target(manager, target)?;
            manager.disable(&plugin.metadata.id)?;
            Ok(PluginsCommandResult {
                message: format!(
                    "Plugins\n  Result           disabled {}\n  Name             {}\n  Version          {}\n  Status           disabled",
                    plugin.metadata.id, plugin.metadata.name, plugin.metadata.version
                ),
                reload_runtime: true,
            })
        }
        Some("uninstall") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins uninstall <plugin-id>".to_string(),
                    reload_runtime: false,
                });
            };
            manager.uninstall(target)?;
            Ok(PluginsCommandResult {
                message: format!("Plugins\n  Result           uninstalled {target}"),
                reload_runtime: true,
            })
        }
        Some("update") => {
            let Some(target) = target else {
                return Ok(PluginsCommandResult {
                    message: "Usage: /plugins update <plugin-id>".to_string(),
                    reload_runtime: false,
                });
            };
            let update = manager.update(target, reporter)?;
            let plugin = manager
                .list_installed_plugins()?
                .into_iter()
                .find(|plugin| plugin.metadata.id == update.plugin_id);
            Ok(PluginsCommandResult {
                message: format!(
                    "Plugins\n  Result           updated {}\n  Name             {}\n  Old version      {}\n  New version      {}\n  Status           {}",
                    update.plugin_id,
                    plugin
                        .as_ref()
                        .map_or_else(|| update.plugin_id.clone(), |plugin| plugin.metadata.name.clone()),
                    update.old_version,
                    update.new_version,
                    plugin
                        .as_ref()
                        .map_or("unknown", |plugin| if plugin.enabled { "enabled" } else { "disabled" }),
                ),
                reload_runtime: true,
            })
        }
        Some(other) => Ok(PluginsCommandResult {
            message: format!(
                "Unknown /plugins action '{other}'. Use list, install, enable, disable, uninstall, or update."
            ),
            reload_runtime: false,
        }),
    }
}

pub fn handle_agents_slash_command(args: Option<&str>, cwd: &Path) -> std::io::Result<String> {
    match normalize_optional_args(args) {
        None | Some("list") => {
            let roots = discover_definition_roots(cwd, "agents");
            let agents = load_agents_from_roots(&roots)?;
            Ok(render_agents_report(&agents))
        }
        Some("-h" | "--help" | "help") => Ok(render_agents_usage(None)),
        Some(args) => Ok(render_agents_usage(Some(args))),
    }
}

pub fn handle_skills_slash_command(args: Option<&str>, cwd: &Path) -> std::io::Result<String> {
    match normalize_optional_args(args) {
        None | Some("list") => {
            let roots = discover_skill_roots(cwd);
            let skills = load_skills_from_roots(&roots)?;
            Ok(render_skills_report(&skills))
        }
        Some("-h" | "--help" | "help") => Ok(render_skills_usage(None)),
        Some(args) => Ok(render_skills_usage(Some(args))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPushPrRequest {
    pub commit_message: Option<String>,
    pub pr_title: String,
    pub pr_body: String,
    pub branch_name_hint: String,
}

pub fn handle_branch_slash_command(
    action: Option<&str>,
    target: Option<&str>,
    cwd: &Path,
) -> io::Result<String> {
    match normalize_optional_args(action) {
        None | Some("list") => {
            let branches = git_stdout(cwd, &["branch", "--list", "--verbose"])?;
            let trimmed = branches.trim();
            Ok(if trimmed.is_empty() {
                "Branch\n  Result           no branches found".to_string()
            } else {
                format!("Branch\n  Result           listed\n\n{trimmed}")
            })
        }
        Some("create") => {
            let Some(target) = target.filter(|value| !value.trim().is_empty()) else {
                return Ok("Usage: /branch create <name>".to_string());
            };
            git_status_ok(cwd, &["switch", "-c", target])?;
            Ok(format!(
                "Branch\n  Result           created and switched\n  Branch           {target}"
            ))
        }
        Some("switch") => {
            let Some(target) = target.filter(|value| !value.trim().is_empty()) else {
                return Ok("Usage: /branch switch <name>".to_string());
            };
            git_status_ok(cwd, &["switch", target])?;
            Ok(format!(
                "Branch\n  Result           switched\n  Branch           {target}"
            ))
        }
        Some(other) => Ok(format!(
            "Unknown /branch action '{other}'. Use /branch list, /branch create <name>, or /branch switch <name>."
        )),
    }
}

pub fn handle_worktree_slash_command(
    action: Option<&str>,
    path: Option<&str>,
    branch: Option<&str>,
    cwd: &Path,
) -> io::Result<String> {
    match normalize_optional_args(action) {
        None | Some("list") => {
            let worktrees = git_stdout(cwd, &["worktree", "list"])?;
            let trimmed = worktrees.trim();
            Ok(if trimmed.is_empty() {
                "Worktree\n  Result           no worktrees found".to_string()
            } else {
                format!("Worktree\n  Result           listed\n\n{trimmed}")
            })
        }
        Some("add") => {
            let Some(path) = path.filter(|value| !value.trim().is_empty()) else {
                return Ok("Usage: /worktree add <path> [branch]".to_string());
            };
            if let Some(branch) = branch.filter(|value| !value.trim().is_empty()) {
                if branch_exists(cwd, branch) {
                    git_status_ok(cwd, &["worktree", "add", path, branch])?;
                } else {
                    git_status_ok(cwd, &["worktree", "add", path, "-b", branch])?;
                }
                Ok(format!(
                    "Worktree\n  Result           added\n  Path             {path}\n  Branch           {branch}"
                ))
            } else {
                git_status_ok(cwd, &["worktree", "add", path])?;
                Ok(format!(
                    "Worktree\n  Result           added\n  Path             {path}"
                ))
            }
        }
        Some("remove") => {
            let Some(path) = path.filter(|value| !value.trim().is_empty()) else {
                return Ok("Usage: /worktree remove <path>".to_string());
            };
            git_status_ok(cwd, &["worktree", "remove", path])?;
            Ok(format!(
                "Worktree\n  Result           removed\n  Path             {path}"
            ))
        }
        Some("prune") => {
            git_status_ok(cwd, &["worktree", "prune"])?;
            Ok("Worktree\n  Result           pruned".to_string())
        }
        Some(other) => Ok(format!(
            "Unknown /worktree action '{other}'. Use /worktree list, /worktree add <path> [branch], /worktree remove <path>, or /worktree prune."
        )),
    }
}

pub fn handle_commit_slash_command(
    message: &str,
    cwd: &Path,
    reporter: &dyn ProgressReporter,
) -> io::Result<String> {
    let status = git_stdout(cwd, &["status", "--short"])?;
    if status.trim().is_empty() {
        return Ok(
            "Commit\n  Result           skipped\n  Reason           no workspace changes"
                .to_string(),
        );
    }

    let message = message.trim();
    if message.is_empty() {
        return Err(io::Error::other("generated commit message was empty"));
    }

    reporter.report("Staging all changes...");
    git_status_ok(cwd, &["add", "-A"])?;
    reporter.report("Committing...");
    let path = write_temp_text_file("elai-commit-message", "txt", message)?;
    let path_string = path.to_string_lossy().into_owned();
    git_status_ok(cwd, &["commit", "--file", path_string.as_str()])?;

    Ok(format!(
        "Commit\n  Result           created\n  Message file     {}\n\n{}",
        path.display(),
        message
    ))
}

#[allow(clippy::too_many_lines)]
pub fn handle_commit_push_pr_slash_command(
    request: &CommitPushPrRequest,
    cwd: &Path,
    reporter: &dyn ProgressReporter,
) -> io::Result<String> {
    reporter.report("Checking prerequisites...");
    if !command_exists("gh") {
        return Err(io::Error::other("gh CLI is required for /commit-push-pr"));
    }

    let default_branch = detect_default_branch(cwd)?;
    reporter.report(&format!("Detected base branch: {default_branch}"));
    let mut branch = current_branch(cwd)?;
    let mut created_branch = false;
    if branch == default_branch {
        let hint = if request.branch_name_hint.trim().is_empty() {
            request.pr_title.as_str()
        } else {
            request.branch_name_hint.as_str()
        };
        let next_branch = build_branch_name(hint);
        git_status_ok(cwd, &["switch", "-c", next_branch.as_str()])?;
        branch.clone_from(&next_branch);
        reporter.report(&format!("Created branch: {next_branch}"));
        created_branch = true;
    }

    reporter.report("Checking workspace changes...");
    let workspace_has_changes = !git_stdout(cwd, &["status", "--short"])?.trim().is_empty();
    let commit_report = if workspace_has_changes {
        let Some(message) = request.commit_message.as_deref() else {
            return Err(io::Error::other(
                "commit message is required when workspace changes are present",
            ));
        };
        reporter.report("Committing changes...");
        Some(handle_commit_slash_command(message, cwd, reporter)?)
    } else {
        None
    };

    reporter.report("Verifying branch diff...");
    let branch_diff = git_stdout(
        cwd,
        &["diff", "--stat", &format!("{default_branch}...HEAD")],
    )?;
    if branch_diff.trim().is_empty() {
        return Ok(
            "Commit/Push/PR\n  Result           skipped\n  Reason           no branch changes to push or open as a pull request"
                .to_string(),
        );
    }

    reporter.report(&format!("Pushing to origin/{branch}..."));
    git_status_ok(cwd, &["push", "--set-upstream", "origin", branch.as_str()])?;

    reporter.report("Creating pull request...");
    let body_path = write_temp_text_file("elai-pr-body", "md", request.pr_body.trim())?;
    let body_path_string = body_path.to_string_lossy().into_owned();
    let create = Command::new("gh")
        .args([
            "pr",
            "create",
            "--title",
            request.pr_title.as_str(),
            "--body-file",
            body_path_string.as_str(),
            "--base",
            default_branch.as_str(),
        ])
        .current_dir(cwd)
        .output()?;

    let (result, url) = if create.status.success() {
        (
            "created",
            parse_pr_url(&String::from_utf8_lossy(&create.stdout))
                .unwrap_or_else(|| "<unknown>".to_string()),
        )
    } else {
        let view = Command::new("gh")
            .args(["pr", "view", "--json", "url"])
            .current_dir(cwd)
            .output()?;
        if !view.status.success() {
            return Err(io::Error::other(command_failure(
                "gh",
                &["pr", "create"],
                &create,
            )));
        }
        (
            "existing",
            parse_pr_json_url(&String::from_utf8_lossy(&view.stdout))
                .unwrap_or_else(|| "<unknown>".to_string()),
        )
    };

    let mut lines = vec![
        "Commit/Push/PR".to_string(),
        format!("  Result           {result}"),
        format!("  Branch           {branch}"),
        format!("  Base             {default_branch}"),
        format!("  Body file        {}", body_path.display()),
        format!("  URL              {url}"),
    ];
    if created_branch {
        lines.insert(2, "  Branch action    created and switched".to_string());
    }
    if let Some(report) = commit_report {
        lines.push(String::new());
        lines.push(report);
    }
    reporter.report(&format!("PR created: {url}"));
    Ok(lines.join("\n"))
}

pub fn detect_default_branch(cwd: &Path) -> io::Result<String> {
    if let Ok(reference) = git_stdout(cwd, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        if let Some(branch) = reference
            .trim()
            .rsplit('/')
            .next()
            .filter(|value| !value.is_empty())
        {
            return Ok(branch.to_string());
        }
    }

    for branch in ["main", "master"] {
        if branch_exists(cwd, branch) {
            return Ok(branch.to_string());
        }
    }

    current_branch(cwd)
}

fn git_stdout(cwd: &Path, args: &[&str]) -> io::Result<String> {
    run_command_stdout("git", args, cwd)
}

fn git_status_ok(cwd: &Path, args: &[&str]) -> io::Result<()> {
    run_command_success("git", args, cwd)
}

fn run_command_stdout(program: &str, args: &[&str], cwd: &Path) -> io::Result<String> {
    let output = Command::new(program).args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        return Err(io::Error::other(command_failure(program, args, &output)));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn run_command_success(program: &str, args: &[&str], cwd: &Path) -> io::Result<()> {
    let output = Command::new(program).args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        return Err(io::Error::other(command_failure(program, args, &output)));
    }
    Ok(())
}

fn command_failure(program: &str, args: &[&str], output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    if detail.is_empty() {
        format!("{program} {} failed", args.join(" "))
    } else {
        format!("{program} {} failed: {detail}", args.join(" "))
    }
}

fn branch_exists(cwd: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(cwd)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn current_branch(cwd: &Path) -> io::Result<String> {
    let branch = git_stdout(cwd, &["branch", "--show-current"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        Err(io::Error::other("unable to determine current git branch"))
    } else {
        Ok(branch.to_string())
    }
}

fn command_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn write_temp_text_file(prefix: &str, extension: &str, contents: &str) -> io::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = env::temp_dir().join(format!("{prefix}-{nanos}.{extension}"));
    fs::write(&path, contents)?;
    Ok(path)
}

fn build_branch_name(hint: &str) -> String {
    let slug = slugify(hint);
    let owner = env::var("SAFEUSER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
    match owner {
        Some(owner) => format!("{owner}/{slug}"),
        None => slug,
    }
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "change".to_string()
    } else {
        slug
    }
}

fn parse_pr_url(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("http://") || line.starts_with("https://"))
        .map(ToOwned::to_owned)
}

fn parse_pr_json_url(stdout: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(stdout)
        .ok()?
        .get("url")?
        .as_str()
        .map(ToOwned::to_owned)
}

#[must_use]
pub fn render_plugins_report(plugins: &[PluginSummary]) -> String {
    let mut lines = vec!["Plugins".to_string()];
    if plugins.is_empty() {
        lines.push("  No plugins installed.".to_string());
        return lines.join("\n");
    }
    for plugin in plugins {
        let enabled = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        lines.push(format!(
            "  {name:<20} v{version:<10} {enabled}",
            name = plugin.metadata.name,
            version = plugin.metadata.version,
        ));
    }
    lines.join("\n")
}

fn render_plugin_install_report(plugin_id: &str, plugin: Option<&PluginSummary>) -> String {
    let name = plugin.map_or(plugin_id, |plugin| plugin.metadata.name.as_str());
    let version = plugin.map_or("unknown", |plugin| plugin.metadata.version.as_str());
    let enabled = plugin.is_some_and(|plugin| plugin.enabled);
    format!(
        "Plugins\n  Result           installed {plugin_id}\n  Name             {name}\n  Version          {version}\n  Status           {}",
        if enabled { "enabled" } else { "disabled" }
    )
}

fn resolve_plugin_target(
    manager: &PluginManager,
    target: &str,
) -> Result<PluginSummary, PluginError> {
    let mut matches = manager
        .list_installed_plugins()?
        .into_iter()
        .filter(|plugin| plugin.metadata.id == target || plugin.metadata.name == target)
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(PluginError::NotFound(format!(
            "plugin `{target}` is not installed or discoverable"
        ))),
        _ => Err(PluginError::InvalidManifest(format!(
            "plugin name `{target}` is ambiguous; use the full plugin id"
        ))),
    }
}

fn discover_definition_roots(cwd: &Path, leaf: &str) -> Vec<(DefinitionSource, PathBuf)> {
    let mut roots = Vec::new();

    for ancestor in cwd.ancestors() {
        push_unique_root(
            &mut roots,
            DefinitionSource::ProjectCodex,
            ancestor.join(".codex").join(leaf),
        );
        push_unique_root(
            &mut roots,
            DefinitionSource::ProjectElai,
            ancestor.join(".elai").join(leaf),
        );
    }

    if let Ok(codex_home) = env::var("CODEX_HOME") {
        push_unique_root(
            &mut roots,
            DefinitionSource::UserCodexHome,
            PathBuf::from(codex_home).join(leaf),
        );
    }

    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        push_unique_root(
            &mut roots,
            DefinitionSource::UserCodex,
            home.join(".codex").join(leaf),
        );
        push_unique_root(
            &mut roots,
            DefinitionSource::UserElai,
            home.join(".elai").join(leaf),
        );
    }

    roots
}

fn discover_skill_roots(cwd: &Path) -> Vec<SkillRoot> {
    let mut roots = Vec::new();

    for ancestor in cwd.ancestors() {
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::ProjectCodex,
            ancestor.join(".codex").join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::ProjectElai,
            ancestor.join(".elai").join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::ProjectCodex,
            ancestor.join(".codex").join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::ProjectElai,
            ancestor.join(".elai").join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
    }

    if let Ok(codex_home) = env::var("CODEX_HOME") {
        let codex_home = PathBuf::from(codex_home);
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserCodexHome,
            codex_home.join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserCodexHome,
            codex_home.join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
    }

    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserCodex,
            home.join(".codex").join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserCodex,
            home.join(".codex").join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserElai,
            home.join(".elai").join("skills"),
            SkillOrigin::SkillsDir,
        );
        push_unique_skill_root(
            &mut roots,
            DefinitionSource::UserElai,
            home.join(".elai").join("commands"),
            SkillOrigin::LegacyCommandsDir,
        );
    }

    roots
}

fn push_unique_root(
    roots: &mut Vec<(DefinitionSource, PathBuf)>,
    source: DefinitionSource,
    path: PathBuf,
) {
    if path.is_dir() && !roots.iter().any(|(_, existing)| existing == &path) {
        roots.push((source, path));
    }
}

fn push_unique_skill_root(
    roots: &mut Vec<SkillRoot>,
    source: DefinitionSource,
    path: PathBuf,
    origin: SkillOrigin,
) {
    if path.is_dir() && !roots.iter().any(|existing| existing.path == path) {
        roots.push(SkillRoot {
            source,
            path,
            origin,
        });
    }
}

fn load_agents_from_roots(
    roots: &[(DefinitionSource, PathBuf)],
) -> std::io::Result<Vec<AgentSummary>> {
    let mut agents = Vec::new();
    let mut active_sources = BTreeMap::<String, DefinitionSource>::new();

    for (source, root) in roots {
        let mut root_agents = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if entry.path().extension().is_none_or(|ext| ext != "toml") {
                continue;
            }
            let contents = fs::read_to_string(entry.path())?;
            let fallback_name = entry.path().file_stem().map_or_else(
                || entry.file_name().to_string_lossy().to_string(),
                |stem| stem.to_string_lossy().to_string(),
            );
            root_agents.push(AgentSummary {
                name: parse_toml_string(&contents, "name").unwrap_or(fallback_name),
                description: parse_toml_string(&contents, "description"),
                model: parse_toml_string(&contents, "model"),
                reasoning_effort: parse_toml_string(&contents, "model_reasoning_effort"),
                source: *source,
                shadowed_by: None,
            });
        }
        root_agents.sort_by(|left, right| left.name.cmp(&right.name));

        for mut agent in root_agents {
            let key = agent.name.to_ascii_lowercase();
            if let Some(existing) = active_sources.get(&key) {
                agent.shadowed_by = Some(*existing);
            } else {
                active_sources.insert(key, agent.source);
            }
            agents.push(agent);
        }
    }

    Ok(agents)
}

fn load_skills_from_roots(roots: &[SkillRoot]) -> std::io::Result<Vec<SkillSummary>> {
    let mut skills = Vec::new();
    let mut active_sources = BTreeMap::<String, DefinitionSource>::new();

    for root in roots {
        let mut root_skills = Vec::new();
        for entry in fs::read_dir(&root.path)? {
            let entry = entry?;
            match root.origin {
                SkillOrigin::SkillsDir => {
                    if !entry.path().is_dir() {
                        continue;
                    }
                    let skill_path = entry.path().join("SKILL.md");
                    if !skill_path.is_file() {
                        continue;
                    }
                    let contents = fs::read_to_string(skill_path)?;
                    let parsed = parse_skill_frontmatter(&contents);
                    root_skills.push(SkillSummary {
                        name: parsed.name.unwrap_or_else(|| {
                            entry.file_name().to_string_lossy().to_string()
                        }),
                        description: parsed.description,
                        source: root.source,
                        shadowed_by: None,
                        origin: root.origin,
                        priority: parsed.priority,
                        budget_multiplier: parsed.budget_multiplier,
                        force_provider: parsed.force_provider,
                        incompatible_with: parsed.incompatible_with,
                    });
                }
                SkillOrigin::LegacyCommandsDir => {
                    let path = entry.path();
                    let markdown_path = if path.is_dir() {
                        let skill_path = path.join("SKILL.md");
                        if !skill_path.is_file() {
                            continue;
                        }
                        skill_path
                    } else if path
                        .extension()
                        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("md"))
                    {
                        path
                    } else {
                        continue;
                    };

                    let contents = fs::read_to_string(&markdown_path)?;
                    let fallback_name = markdown_path.file_stem().map_or_else(
                        || entry.file_name().to_string_lossy().to_string(),
                        |stem| stem.to_string_lossy().to_string(),
                    );
                    let parsed = parse_skill_frontmatter(&contents);
                    root_skills.push(SkillSummary {
                        name: parsed.name.unwrap_or(fallback_name),
                        description: parsed.description,
                        source: root.source,
                        shadowed_by: None,
                        origin: root.origin,
                        priority: parsed.priority,
                        budget_multiplier: parsed.budget_multiplier,
                        force_provider: parsed.force_provider,
                        incompatible_with: parsed.incompatible_with,
                    });
                }
            }
        }
        root_skills.sort_by(|left, right| left.name.cmp(&right.name));

        for mut skill in root_skills {
            let key = skill.name.to_ascii_lowercase();
            if let Some(existing) = active_sources.get(&key) {
                skill.shadowed_by = Some(*existing);
            } else {
                active_sources.insert(key, skill.source);
            }
            skills.push(skill);
        }
    }

    Ok(skills)
}

fn parse_toml_string(contents: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} =");
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(value) = trimmed.strip_prefix(&prefix) else {
            continue;
        };
        let value = value.trim();
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            continue;
        };
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

struct ParsedSkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    priority: Option<i32>,
    budget_multiplier: Option<f32>,
    force_provider: Option<String>,
    incompatible_with: Vec<String>,
}

fn parse_skill_frontmatter(contents: &str) -> ParsedSkillFrontmatter {
    let lines: Vec<&str> = contents.lines().collect();
    let mut out = ParsedSkillFrontmatter {
        name: None,
        description: None,
        priority: None,
        budget_multiplier: None,
        force_provider: None,
        incompatible_with: Vec::new(),
    };

    if lines.first().map(|s| s.trim()) != Some("---") {
        return out;
    }

    let mut i = 1;
    let mut in_incompat_list = false;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed == "---" {
            break;
        }

        if in_incompat_list {
            if let Some(item) = trimmed.strip_prefix("- ") {
                out.incompatible_with
                    .push(unquote_frontmatter_value(item.trim()));
                i += 1;
                continue;
            }
            in_incompat_list = false;
        }

        if let Some(value) = trimmed.strip_prefix("name:") {
            let v = unquote_frontmatter_value(value.trim());
            if !v.is_empty() {
                out.name = Some(v);
            }
        } else if let Some(value) = trimmed.strip_prefix("description:") {
            let v = unquote_frontmatter_value(value.trim());
            if !v.is_empty() {
                out.description = Some(v);
            }
        } else if let Some(value) = trimmed.strip_prefix("priority:") {
            out.priority = value.trim().parse::<i32>().ok();
        } else if let Some(value) = trimmed.strip_prefix("budget_multiplier:") {
            out.budget_multiplier = value.trim().parse::<f32>().ok();
        } else if let Some(value) = trimmed.strip_prefix("force_provider:") {
            let v = unquote_frontmatter_value(value.trim());
            if !v.is_empty() {
                out.force_provider = Some(v);
            }
        } else if trimmed.starts_with("incompatible_with:") {
            // Check for inline single value: `incompatible_with: skill-a`
            let after_colon = trimmed
                .strip_prefix("incompatible_with:")
                .unwrap_or("")
                .trim();
            if after_colon.is_empty() {
                in_incompat_list = true;
            } else {
                out.incompatible_with
                    .push(unquote_frontmatter_value(after_colon));
            }
        }

        i += 1;
    }

    out
}

fn unquote_frontmatter_value(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|trimmed| trimmed.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|trimmed| trimmed.strip_suffix('\''))
        })
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn render_agents_report(agents: &[AgentSummary]) -> String {
    if agents.is_empty() {
        return "No agents found.".to_string();
    }

    let total_active = agents
        .iter()
        .filter(|agent| agent.shadowed_by.is_none())
        .count();
    let mut lines = vec![
        "Agents".to_string(),
        format!("  {total_active} active agents"),
        String::new(),
    ];

    for source in [
        DefinitionSource::ProjectCodex,
        DefinitionSource::ProjectElai,
        DefinitionSource::UserCodexHome,
        DefinitionSource::UserCodex,
        DefinitionSource::UserElai,
    ] {
        let group = agents
            .iter()
            .filter(|agent| agent.source == source)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }

        lines.push(format!("{}:", source.label()));
        for agent in group {
            let detail = agent_detail(agent);
            match agent.shadowed_by {
                Some(winner) => lines.push(format!("  (shadowed by {}) {detail}", winner.label())),
                None => lines.push(format!("  {detail}")),
            }
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_string()
}

fn agent_detail(agent: &AgentSummary) -> String {
    let mut parts = vec![agent.name.clone()];
    if let Some(description) = &agent.description {
        parts.push(description.clone());
    }
    if let Some(model) = &agent.model {
        parts.push(model.clone());
    }
    if let Some(reasoning) = &agent.reasoning_effort {
        parts.push(reasoning.clone());
    }
    parts.join(" · ")
}

fn render_skills_report(skills: &[SkillSummary]) -> String {
    if skills.is_empty() {
        return "No skills found.".to_string();
    }

    let total_active = skills
        .iter()
        .filter(|skill| skill.shadowed_by.is_none())
        .count();
    let mut lines = vec![
        "Skills".to_string(),
        format!("  {total_active} available skills"),
        String::new(),
    ];

    for source in [
        DefinitionSource::ProjectCodex,
        DefinitionSource::ProjectElai,
        DefinitionSource::UserCodexHome,
        DefinitionSource::UserCodex,
        DefinitionSource::UserElai,
    ] {
        let group = skills
            .iter()
            .filter(|skill| skill.source == source)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }

        lines.push(format!("{}:", source.label()));
        for skill in group {
            let mut parts = vec![skill.name.clone()];
            if let Some(description) = &skill.description {
                parts.push(description.clone());
            }
            if let Some(detail) = skill.origin.detail_label() {
                parts.push(detail.to_string());
            }
            let detail = parts.join(" · ");
            let line = match skill.shadowed_by {
                Some(winner) => format!("  (shadowed by {}) {detail}", winner.label()),
                None => format!("  {detail}"),
            };
            lines.push(line);

            // Extra metadata on the next indented line
            let mut meta_parts = Vec::new();
            if let Some(p) = skill.priority {
                meta_parts.push(format!("priority={p}"));
            }
            if let Some(m) = skill.budget_multiplier {
                if (m - 1.0).abs() > f32::EPSILON {
                    meta_parts.push(format!("budget={m}x"));
                }
            }
            if let Some(fp) = &skill.force_provider {
                meta_parts.push(format!("provider={fp}"));
            }
            if !skill.incompatible_with.is_empty() {
                meta_parts.push(format!("incompatible={}", skill.incompatible_with.join(",")));
            }
            if !meta_parts.is_empty() {
                lines.push(format!("    [{}]", meta_parts.join(" | ")));
            }
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_string()
}

fn normalize_optional_args(args: Option<&str>) -> Option<&str> {
    args.map(str::trim).filter(|value| !value.is_empty())
}

fn render_agents_usage(unexpected: Option<&str>) -> String {
    let mut lines = vec![
        "Agents".to_string(),
        "  Usage            /agents".to_string(),
        "  Direct CLI       elai agents".to_string(),
        "  Sources          .codex/agents, .elai/agents, $CODEX_HOME/agents".to_string(),
    ];
    if let Some(args) = unexpected {
        lines.push(format!("  Unexpected       {args}"));
    }
    lines.join("\n")
}

fn render_skills_usage(unexpected: Option<&str>) -> String {
    let mut lines = vec![
        "Skills".to_string(),
        "  Usage            /skills".to_string(),
        "  Direct CLI       elai skills".to_string(),
        "  Sources          .codex/skills, .elai/skills, legacy /commands".to_string(),
    ];
    if let Some(args) = unexpected {
        lines.push(format!("  Unexpected       {args}"));
    }
    lines.join("\n")
}

#[must_use]
pub fn handle_slash_command(
    input: &str,
    session: &Session,
    compaction: CompactionConfig,
) -> Option<SlashCommandResult> {
    match SlashCommand::parse(input)? {
        SlashCommand::Compact => {
            let result = compact_session(session, compaction);
            let message = if result.removed_message_count == 0 {
                "Compaction skipped: session is below the compaction threshold.".to_string()
            } else {
                format!(
                    "Compacted {} messages into a resumable system summary.",
                    result.removed_message_count
                )
            };
            Some(SlashCommandResult {
                message,
                session: result.compacted_session,
            })
        }
        SlashCommand::Help => Some(SlashCommandResult {
            message: render_help_grouped(),
            session: session.clone(),
        }),
        SlashCommand::Tools { subcommand } => Some(SlashCommandResult {
            message: handle_tools_slash_command(subcommand.as_deref()),
            session: session.clone(),
        }),
        SlashCommand::Status
        | SlashCommand::Branch { .. }
        | SlashCommand::Bughunter { .. }
        | SlashCommand::Worktree { .. }
        | SlashCommand::Commit
        | SlashCommand::CommitPushPr { .. }
        | SlashCommand::Pr { .. }
        | SlashCommand::Issue { .. }
        | SlashCommand::Ultraplan { .. }
        | SlashCommand::Teleport { .. }
        | SlashCommand::DebugToolCall
        | SlashCommand::Model { .. }
        | SlashCommand::Permissions { .. }
        | SlashCommand::Clear { .. }
        | SlashCommand::Cost
        | SlashCommand::Resume { .. }
        | SlashCommand::Config { .. }
        | SlashCommand::Memory
        | SlashCommand::Init
        | SlashCommand::Diff
        | SlashCommand::Version
        | SlashCommand::Update
        | SlashCommand::Export { .. }
        | SlashCommand::Session { .. }
        | SlashCommand::Plugins { .. }
        | SlashCommand::Agents { .. }
        | SlashCommand::Skills { .. }
        | SlashCommand::Budget { .. }
        | SlashCommand::Cache { .. }
        | SlashCommand::Dream { .. }
        | SlashCommand::Stats { .. }
        | SlashCommand::Providers { .. }
        | SlashCommand::Verify
        | SlashCommand::Locale { .. }
        | SlashCommand::Unknown(_) => None,
        SlashCommand::Run { script, args, update } => {
            use script_runner::{run_script, ScriptConfig, NullReporter};
            let result = run_script(
                &script,
                args.as_deref(),
                ScriptConfig { update },
                &NullReporter,
            );
            let msg = match result {
                Ok(output) => format!(
                    "Run\n  Script           {script}\n  Args             {}\n  Update           {}\n\n{}",
                    args.as_deref().unwrap_or("none"),
                    update,
                    output
                ),
                Err(e) => format!("Run\n  Error            {e}"),
            };
            Some(SlashCommandResult {
                message: msg,
                session: session.clone(),
            })
        }
    }
}

/// Handles the `/tools` slash command and its sub-commands.
///
/// - `/tools` or `/tools list` — placeholder (full wiring in a future integration phase)
/// - `/tools why` — lists tools rejected in the last pipeline run with their reason
pub fn handle_tools_slash_command(subcommand: Option<&str>) -> String {
    use runtime::{last_rejected, RejectionReason};

    match subcommand.map(str::trim) {
        Some("why") => {
            let rejected = last_rejected();
            if rejected.is_empty() {
                return "Tools\n  No tools were rejected in the last turn (or no turn has run yet).".to_string();
            }
            let mut lines = vec![
                "Tools — rejected in last turn".to_string(),
                String::new(),
                format!("  {:<40} {}", "Tool", "Reason"),
                format!("  {:<40} {}", "----", "------"),
            ];
            for r in &rejected {
                let reason = match &r.reason {
                    RejectionReason::Disabled => "disabled in catalog".to_string(),
                    RejectionReason::SkillIncompatible(skill) => {
                        format!("incompatible with skill '{skill}'")
                    }
                    RejectionReason::UserFilter => "--allowedTools filter".to_string(),
                    RejectionReason::BudgetCap => "budget cap (top-N)".to_string(),
                };
                lines.push(format!("  {:<40} {reason}", r.id));
            }
            lines.join("\n")
        }
        None | Some("list" | "") => {
            // Future: list active tools in current session.
            "Tools\n  Use `/tools why` to see rejected tools from the last turn.".to_string()
        }
        Some(other) => format!(
            "Unknown /tools subcommand '{other}'. Use `/tools why` to see rejected tools."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        handle_branch_slash_command, handle_commit_push_pr_slash_command,
        handle_commit_slash_command, handle_plugins_slash_command, handle_slash_command,
        handle_worktree_slash_command, load_agents_from_roots, load_skills_from_roots,
        render_agents_report, render_plugins_report, render_skills_report,
        render_slash_command_help, resume_supported_slash_commands, slash_command_specs,
        CommitPushPrRequest, DefinitionSource, SkillOrigin, SkillRoot, SlashCommand,
    };
    use plugins::{PluginKind, PluginManager, PluginManagerConfig, PluginMetadata, PluginSummary};
    use runtime::{CompactionConfig, ContentBlock, ConversationMessage, MessageRole, Session};
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("commands-plugin-{label}-{nanos}"))
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock")
    }

    fn run_command(cwd: &Path, program: &str, args: &[&str]) -> String {
        let output = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("command should run");
        assert!(
            output.status.success(),
            "{} {} failed: {}",
            program,
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout should be utf8")
    }

    fn init_git_repo(label: &str) -> PathBuf {
        let root = temp_dir(label);
        fs::create_dir_all(&root).expect("repo root");

        let init = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&root)
            .output()
            .expect("git init should run");
        if !init.status.success() {
            let fallback = Command::new("git")
                .arg("init")
                .current_dir(&root)
                .output()
                .expect("fallback git init should run");
            assert!(
                fallback.status.success(),
                "fallback git init should succeed"
            );
            let rename = Command::new("git")
                .args(["branch", "-m", "main"])
                .current_dir(&root)
                .output()
                .expect("git branch -m should run");
            assert!(rename.status.success(), "git branch -m main should succeed");
        }

        run_command(&root, "git", &["config", "user.name", "Elai Tests"]);
        run_command(&root, "git", &["config", "user.email", "elai@example.com"]);
        fs::write(root.join("README.md"), "seed\n").expect("seed file");
        run_command(&root, "git", &["add", "README.md"]);
        run_command(&root, "git", &["commit", "-m", "chore: seed repo"]);
        root
    }

    fn init_bare_repo(label: &str) -> PathBuf {
        let root = temp_dir(label);
        let output = Command::new("git")
            .args(["init", "--bare"])
            .arg(&root)
            .output()
            .expect("bare repo should initialize");
        assert!(output.status.success(), "git init --bare should succeed");
        root
    }

    #[cfg(unix)]
    fn write_fake_gh(bin_dir: &Path, log_path: &Path, url: &str) {
        fs::create_dir_all(bin_dir).expect("bin dir");
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo 'gh 1.0.0'\n  exit 0\nfi\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then\n  echo '{}'\n  exit 0\nfi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then\n  echo '{{\"url\":\"{}\"}}'\n  exit 0\nfi\nexit 0\n",
            log_path.display(),
            url,
            url,
        );
        let path = bin_dir.join("gh");
        fs::write(&path, script).expect("gh stub");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
    }

    fn write_external_plugin(root: &Path, name: &str, version: &str) {
        fs::create_dir_all(root.join(".elai-plugin")).expect("manifest dir");
        fs::write(
            root.join(".elai-plugin").join("plugin.json"),
            format!(
                "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"commands plugin\"\n}}"
            ),
        )
        .expect("write manifest");
    }

    fn write_bundled_plugin(root: &Path, name: &str, version: &str, default_enabled: bool) {
        fs::create_dir_all(root.join(".elai-plugin")).expect("manifest dir");
        fs::write(
            root.join(".elai-plugin").join("plugin.json"),
            format!(
                "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"bundled commands plugin\",\n  \"defaultEnabled\": {}\n}}",
                if default_enabled { "true" } else { "false" }
            ),
        )
        .expect("write bundled manifest");
    }

    fn write_agent(root: &Path, name: &str, description: &str, model: &str, reasoning: &str) {
        fs::create_dir_all(root).expect("agent root");
        fs::write(
            root.join(format!("{name}.toml")),
            format!(
                "name = \"{name}\"\ndescription = \"{description}\"\nmodel = \"{model}\"\nmodel_reasoning_effort = \"{reasoning}\"\n"
            ),
        )
        .expect("write agent");
    }

    fn write_skill(root: &Path, name: &str, description: &str) {
        let skill_root = root.join(name);
        fs::create_dir_all(&skill_root).expect("skill root");
        fs::write(
            skill_root.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
        )
        .expect("write skill");
    }

    fn write_legacy_command(root: &Path, name: &str, description: &str) {
        fs::create_dir_all(root).expect("commands root");
        fs::write(
            root.join(format!("{name}.md")),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
        )
        .expect("write command");
    }

    #[allow(clippy::too_many_lines)]
    #[test]
    fn parses_supported_slash_commands() {
        assert_eq!(SlashCommand::parse("/help"), Some(SlashCommand::Help));
        assert_eq!(SlashCommand::parse(" /status "), Some(SlashCommand::Status));
        assert_eq!(
            SlashCommand::parse("/bughunter runtime"),
            Some(SlashCommand::Bughunter {
                scope: Some("runtime".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/branch create feature/demo"),
            Some(SlashCommand::Branch {
                action: Some("create".to_string()),
                target: Some("feature/demo".to_string()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/worktree add ../demo wt-demo"),
            Some(SlashCommand::Worktree {
                action: Some("add".to_string()),
                path: Some("../demo".to_string()),
                branch: Some("wt-demo".to_string()),
            })
        );
        assert_eq!(SlashCommand::parse("/commit"), Some(SlashCommand::Commit));
        assert_eq!(
            SlashCommand::parse("/commit-push-pr ready for review"),
            Some(SlashCommand::CommitPushPr {
                context: Some("ready for review".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/pr ready for review"),
            Some(SlashCommand::Pr {
                context: Some("ready for review".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/issue flaky test"),
            Some(SlashCommand::Issue {
                context: Some("flaky test".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/ultraplan ship both features"),
            Some(SlashCommand::Ultraplan {
                task: Some("ship both features".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/teleport conversation.rs"),
            Some(SlashCommand::Teleport {
                target: Some("conversation.rs".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/debug-tool-call"),
            Some(SlashCommand::DebugToolCall)
        );
        assert_eq!(
            SlashCommand::parse("/model opus"),
            Some(SlashCommand::Model {
                model: Some("opus".to_string()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/model"),
            Some(SlashCommand::Model { model: None })
        );
        assert_eq!(
            SlashCommand::parse("/permissions read-only"),
            Some(SlashCommand::Permissions {
                mode: Some("read-only".to_string()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/clear"),
            Some(SlashCommand::Clear { confirm: false })
        );
        assert_eq!(
            SlashCommand::parse("/clear --confirm"),
            Some(SlashCommand::Clear { confirm: true })
        );
        assert_eq!(SlashCommand::parse("/cost"), Some(SlashCommand::Cost));
        assert_eq!(
            SlashCommand::parse("/resume session.json"),
            Some(SlashCommand::Resume {
                session_path: Some("session.json".to_string()),
            })
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
        assert_eq!(SlashCommand::parse("/diff"), Some(SlashCommand::Diff));
        assert_eq!(SlashCommand::parse("/version"), Some(SlashCommand::Version));
        assert_eq!(SlashCommand::parse("/update"), Some(SlashCommand::Update));
        assert_eq!(
            SlashCommand::parse("/export notes.txt"),
            Some(SlashCommand::Export {
                path: Some("notes.txt".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/session switch abc123"),
            Some(SlashCommand::Session {
                action: Some("switch".to_string()),
                target: Some("abc123".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/plugins install demo"),
            Some(SlashCommand::Plugins {
                action: Some("install".to_string()),
                target: Some("demo".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/plugins list"),
            Some(SlashCommand::Plugins {
                action: Some("list".to_string()),
                target: None
            })
        );
        assert_eq!(
            SlashCommand::parse("/plugins enable demo"),
            Some(SlashCommand::Plugins {
                action: Some("enable".to_string()),
                target: Some("demo".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/plugins disable demo"),
            Some(SlashCommand::Plugins {
                action: Some("disable".to_string()),
                target: Some("demo".to_string())
            })
        );
    }

    #[test]
    fn renders_help_from_shared_specs() {
        // Fixa locale=en para os asserts contra `argument_hint` em inglês continuarem
        // estáveis. Locale é global por processo no rust-i18n, portanto se outro
        // teste paralelo trocar o locale pode haver flake — mover para mutex global
        // se isso ocorrer.
        rust_i18n::set_locale("en");

        let help = render_slash_command_help();
        assert!(help.contains("works with --resume SESSION.json"));
        assert!(help.contains("/help"));
        assert!(help.contains("/status"));
        assert!(help.contains("/compact"));
        assert!(help.contains("/bughunter [scope]"));
        assert!(help.contains("/branch [list|create <name>|switch <name>]"));
        assert!(help.contains("/worktree [list|add <path> [branch]|remove <path>|prune]"));
        assert!(help.contains("/commit"));
        assert!(help.contains("/commit-push-pr [context]"));
        assert!(help.contains("/pr [context]"));
        assert!(help.contains("/issue [context]"));
        assert!(help.contains("/ultraplan [task]"));
        assert!(help.contains("/teleport <symbol-or-path>"));
        assert!(help.contains("/debug-tool-call"));
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
        assert!(help.contains("/locale [pt-BR|en]"));
        assert_eq!(slash_command_specs().len(), 37);
        assert_eq!(resume_supported_slash_commands().len(), 19);
    }

    #[test]
    fn compacts_sessions_via_slash_command() {
        let session = Session {
            version: 1,
            messages: vec![
                ConversationMessage::user_text("a ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "b ".repeat(200),
                }]),
                ConversationMessage::tool_result("1", "bash", "ok ".repeat(200), false),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "recent".to_string(),
                }]),
            ],
        };

        let result = handle_slash_command(
            "/compact",
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
            },
        )
        .expect("slash command should be handled");

        assert!(result.message.contains("Compacted 2 messages"));
        assert_eq!(result.session.messages[0].role, MessageRole::System);
    }

    #[test]
    fn help_command_is_non_mutating() {
        let session = Session::new();
        let result = handle_slash_command("/help", &session, CompactionConfig::default())
            .expect("help command should be handled");
        assert_eq!(result.session, session);
        assert!(result.message.contains("Slash commands"));
    }

    #[test]
    fn ignores_unknown_or_runtime_bound_slash_commands() {
        let session = Session::new();
        assert!(handle_slash_command("/unknown", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/status", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/branch list", &session, CompactionConfig::default()).is_none()
        );
        assert!(
            handle_slash_command("/bughunter", &session, CompactionConfig::default()).is_none()
        );
        assert!(
            handle_slash_command("/worktree list", &session, CompactionConfig::default()).is_none()
        );
        assert!(handle_slash_command("/commit", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command(
            "/commit-push-pr review notes",
            &session,
            CompactionConfig::default()
        )
        .is_none());
        assert!(handle_slash_command("/pr", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/issue", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/ultraplan", &session, CompactionConfig::default()).is_none()
        );
        assert!(
            handle_slash_command("/teleport foo", &session, CompactionConfig::default()).is_none()
        );
        assert!(
            handle_slash_command("/debug-tool-call", &session, CompactionConfig::default())
                .is_none()
        );
        assert!(
            handle_slash_command("/model sonnet", &session, CompactionConfig::default()).is_none()
        );
        assert!(handle_slash_command(
            "/permissions read-only",
            &session,
            CompactionConfig::default()
        )
        .is_none());
        assert!(handle_slash_command("/clear", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/clear --confirm", &session, CompactionConfig::default())
                .is_none()
        );
        assert!(handle_slash_command("/cost", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command(
            "/resume session.json",
            &session,
            CompactionConfig::default()
        )
        .is_none());
        assert!(handle_slash_command("/config", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/config env", &session, CompactionConfig::default()).is_none()
        );
        assert!(handle_slash_command("/diff", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/version", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/export note.txt", &session, CompactionConfig::default())
                .is_none()
        );
        assert!(
            handle_slash_command("/session list", &session, CompactionConfig::default()).is_none()
        );
        assert!(
            handle_slash_command("/plugins list", &session, CompactionConfig::default()).is_none()
        );
    }

    #[test]
    fn renders_plugins_report_with_name_version_and_status() {
        let rendered = render_plugins_report(&[
            PluginSummary {
                metadata: PluginMetadata {
                    id: "demo@external".to_string(),
                    name: "demo".to_string(),
                    version: "1.2.3".to_string(),
                    description: "demo plugin".to_string(),
                    kind: PluginKind::External,
                    source: "demo".to_string(),
                    default_enabled: false,
                    root: None,
                },
                enabled: true,
            },
            PluginSummary {
                metadata: PluginMetadata {
                    id: "sample@external".to_string(),
                    name: "sample".to_string(),
                    version: "0.9.0".to_string(),
                    description: "sample plugin".to_string(),
                    kind: PluginKind::External,
                    source: "sample".to_string(),
                    default_enabled: false,
                    root: None,
                },
                enabled: false,
            },
        ]);

        assert!(rendered.contains("demo"));
        assert!(rendered.contains("v1.2.3"));
        assert!(rendered.contains("enabled"));
        assert!(rendered.contains("sample"));
        assert!(rendered.contains("v0.9.0"));
        assert!(rendered.contains("disabled"));
    }

    #[test]
    fn lists_agents_from_project_and_user_roots() {
        let workspace = temp_dir("agents-workspace");
        let project_agents = workspace.join(".codex").join("agents");
        let user_home = temp_dir("agents-home");
        let user_agents = user_home.join(".codex").join("agents");

        write_agent(
            &project_agents,
            "planner",
            "Project planner",
            "gpt-5.4",
            "medium",
        );
        write_agent(
            &user_agents,
            "planner",
            "User planner",
            "gpt-5.4-mini",
            "high",
        );
        write_agent(
            &user_agents,
            "verifier",
            "Verification agent",
            "gpt-5.4-mini",
            "high",
        );

        let roots = vec![
            (DefinitionSource::ProjectCodex, project_agents),
            (DefinitionSource::UserCodex, user_agents),
        ];
        let report =
            render_agents_report(&load_agents_from_roots(&roots).expect("agent roots should load"));

        assert!(report.contains("Agents"));
        assert!(report.contains("2 active agents"));
        assert!(report.contains("Project (.codex):"));
        assert!(report.contains("planner · Project planner · gpt-5.4 · medium"));
        assert!(report.contains("User (~/.codex):"));
        assert!(report.contains("(shadowed by Project (.codex)) planner · User planner"));
        assert!(report.contains("verifier · Verification agent · gpt-5.4-mini · high"));

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(user_home);
    }

    #[test]
    fn lists_skills_from_project_and_user_roots() {
        let workspace = temp_dir("skills-workspace");
        let project_skills = workspace.join(".codex").join("skills");
        let project_commands = workspace.join(".elai").join("commands");
        let user_home = temp_dir("skills-home");
        let user_skills = user_home.join(".codex").join("skills");

        write_skill(&project_skills, "plan", "Project planning guidance");
        write_legacy_command(&project_commands, "deploy", "Legacy deployment guidance");
        write_skill(&user_skills, "plan", "User planning guidance");
        write_skill(&user_skills, "help", "Help guidance");

        let roots = vec![
            SkillRoot {
                source: DefinitionSource::ProjectCodex,
                path: project_skills,
                origin: SkillOrigin::SkillsDir,
            },
            SkillRoot {
                source: DefinitionSource::ProjectElai,
                path: project_commands,
                origin: SkillOrigin::LegacyCommandsDir,
            },
            SkillRoot {
                source: DefinitionSource::UserCodex,
                path: user_skills,
                origin: SkillOrigin::SkillsDir,
            },
        ];
        let report =
            render_skills_report(&load_skills_from_roots(&roots).expect("skill roots should load"));

        assert!(report.contains("Skills"));
        assert!(report.contains("3 available skills"));
        assert!(report.contains("Project (.codex):"));
        assert!(report.contains("plan · Project planning guidance"));
        assert!(report.contains("Project (.elai):"));
        assert!(report.contains("deploy · Legacy deployment guidance · legacy /commands"));
        assert!(report.contains("User (~/.codex):"));
        assert!(report.contains("(shadowed by Project (.codex)) plan · User planning guidance"));
        assert!(report.contains("help · Help guidance"));

        let _ = fs::remove_dir_all(workspace);
        let _ = fs::remove_dir_all(user_home);
    }

    #[test]
    fn agents_and_skills_usage_support_help_and_unexpected_args() {
        let cwd = temp_dir("slash-usage");

        let agents_help =
            super::handle_agents_slash_command(Some("help"), &cwd).expect("agents help");
        assert!(agents_help.contains("Usage            /agents"));
        assert!(agents_help.contains("Direct CLI       elai agents"));

        let agents_unexpected =
            super::handle_agents_slash_command(Some("show planner"), &cwd).expect("agents usage");
        assert!(agents_unexpected.contains("Unexpected       show planner"));

        let skills_help =
            super::handle_skills_slash_command(Some("--help"), &cwd).expect("skills help");
        assert!(skills_help.contains("Usage            /skills"));
        assert!(skills_help.contains("legacy /commands"));

        let skills_unexpected =
            super::handle_skills_slash_command(Some("show help"), &cwd).expect("skills usage");
        assert!(skills_unexpected.contains("Unexpected       show help"));

        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn parses_quoted_skill_frontmatter_values() {
        let contents = "---\nname: \"hud\"\ndescription: 'Quoted description'\n---\n";
        let parsed = super::parse_skill_frontmatter(contents);
        assert_eq!(parsed.name.as_deref(), Some("hud"));
        assert_eq!(parsed.description.as_deref(), Some("Quoted description"));
    }

    #[test]
    fn installs_plugin_from_path_and_lists_it() {
        let config_home = temp_dir("home");
        let source_root = temp_dir("source");
        write_external_plugin(&source_root, "demo", "1.0.0");

        let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
        let install = handle_plugins_slash_command(
            Some("install"),
            Some(source_root.to_str().expect("utf8 path")),
            &mut manager,
            &runtime::NoopReporter,
        )
        .expect("install command should succeed");
        assert!(install.reload_runtime);
        assert!(install.message.contains("installed demo@external"));
        assert!(install.message.contains("Name             demo"));
        assert!(install.message.contains("Version          1.0.0"));
        assert!(install.message.contains("Status           enabled"));

        let list = handle_plugins_slash_command(Some("list"), None, &mut manager, &runtime::NoopReporter)
            .expect("list command should succeed");
        assert!(!list.reload_runtime);
        assert!(list.message.contains("demo"));
        assert!(list.message.contains("v1.0.0"));
        assert!(list.message.contains("enabled"));

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn enables_and_disables_plugin_by_name() {
        let config_home = temp_dir("toggle-home");
        let source_root = temp_dir("toggle-source");
        write_external_plugin(&source_root, "demo", "1.0.0");

        let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
        handle_plugins_slash_command(
            Some("install"),
            Some(source_root.to_str().expect("utf8 path")),
            &mut manager,
            &runtime::NoopReporter,
        )
        .expect("install command should succeed");

        let disable = handle_plugins_slash_command(Some("disable"), Some("demo"), &mut manager, &runtime::NoopReporter)
            .expect("disable command should succeed");
        assert!(disable.reload_runtime);
        assert!(disable.message.contains("disabled demo@external"));
        assert!(disable.message.contains("Name             demo"));
        assert!(disable.message.contains("Status           disabled"));

        let list = handle_plugins_slash_command(Some("list"), None, &mut manager, &runtime::NoopReporter)
            .expect("list command should succeed");
        assert!(list.message.contains("demo"));
        assert!(list.message.contains("disabled"));

        let enable = handle_plugins_slash_command(Some("enable"), Some("demo"), &mut manager, &runtime::NoopReporter)
            .expect("enable command should succeed");
        assert!(enable.reload_runtime);
        assert!(enable.message.contains("enabled demo@external"));
        assert!(enable.message.contains("Name             demo"));
        assert!(enable.message.contains("Status           enabled"));

        let list = handle_plugins_slash_command(Some("list"), None, &mut manager, &runtime::NoopReporter)
            .expect("list command should succeed");
        assert!(list.message.contains("demo"));
        assert!(list.message.contains("enabled"));

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn lists_auto_installed_bundled_plugins_with_status() {
        let config_home = temp_dir("bundled-home");
        let bundled_root = temp_dir("bundled-root");
        let bundled_plugin = bundled_root.join("starter");
        write_bundled_plugin(&bundled_plugin, "starter", "0.1.0", false);

        let mut config = PluginManagerConfig::new(&config_home);
        config.bundled_root = Some(bundled_root.clone());
        let mut manager = PluginManager::new(config);

        let list = handle_plugins_slash_command(Some("list"), None, &mut manager, &runtime::NoopReporter)
            .expect("list command should succeed");
        assert!(!list.reload_runtime);
        assert!(list.message.contains("starter"));
        assert!(list.message.contains("v0.1.0"));
        assert!(list.message.contains("disabled"));

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(bundled_root);
    }

    #[test]
    fn branch_and_worktree_commands_manage_git_state() {
        // given
        let repo = init_git_repo("branch-worktree");
        let worktree_path = repo
            .parent()
            .expect("repo should have parent")
            .join("branch-worktree-linked");

        // when
        let branch_list =
            handle_branch_slash_command(Some("list"), None, &repo).expect("branch list succeeds");
        let created = handle_branch_slash_command(Some("create"), Some("feature/demo"), &repo)
            .expect("branch create succeeds");
        let switched = handle_branch_slash_command(Some("switch"), Some("main"), &repo)
            .expect("branch switch succeeds");
        let added = handle_worktree_slash_command(
            Some("add"),
            Some(worktree_path.to_str().expect("utf8 path")),
            Some("wt-demo"),
            &repo,
        )
        .expect("worktree add succeeds");
        let listed_worktrees =
            handle_worktree_slash_command(Some("list"), None, None, &repo).expect("list succeeds");
        let removed = handle_worktree_slash_command(
            Some("remove"),
            Some(worktree_path.to_str().expect("utf8 path")),
            None,
            &repo,
        )
        .expect("remove succeeds");

        // then
        assert!(branch_list.contains("main"));
        assert!(created.contains("feature/demo"));
        assert!(switched.contains("main"));
        assert!(added.contains("wt-demo"));
        assert!(listed_worktrees.contains(worktree_path.to_str().expect("utf8 path")));
        assert!(removed.contains("Result           removed"));

        let _ = fs::remove_dir_all(repo);
        let _ = fs::remove_dir_all(worktree_path);
    }

    #[test]
    fn commit_command_stages_and_commits_changes() {
        // given
        let repo = init_git_repo("commit-command");
        fs::write(repo.join("notes.txt"), "hello\n").expect("write notes");

        // when
        let report =
            handle_commit_slash_command("feat: add notes", &repo, &runtime::NoopReporter).expect("commit succeeds");
        let status = run_command(&repo, "git", &["status", "--short"]);
        let message = run_command(&repo, "git", &["log", "-1", "--pretty=%B"]);

        // then
        assert!(report.contains("Result           created"));
        assert!(status.trim().is_empty());
        assert_eq!(message.trim(), "feat: add notes");

        let _ = fs::remove_dir_all(repo);
    }

    #[cfg(unix)]
    #[test]
    fn commit_push_pr_command_commits_pushes_and_creates_pr() {
        // given
        let _guard = env_lock();
        let repo = init_git_repo("commit-push-pr");
        let remote = init_bare_repo("commit-push-pr-remote");
        run_command(
            &repo,
            "git",
            &[
                "remote",
                "add",
                "origin",
                remote.to_str().expect("utf8 remote"),
            ],
        );
        run_command(&repo, "git", &["push", "-u", "origin", "main"]);
        fs::write(repo.join("feature.txt"), "feature\n").expect("write feature file");

        let fake_bin = temp_dir("fake-gh-bin");
        let gh_log = fake_bin.join("gh.log");
        write_fake_gh(&fake_bin, &gh_log, "https://example.com/pr/123");

        let previous_path = env::var_os("PATH");
        let mut new_path = fake_bin.display().to_string();
        if let Some(path) = &previous_path {
            new_path.push(':');
            new_path.push_str(&path.to_string_lossy());
        }
        env::set_var("PATH", &new_path);
        let previous_safeuser = env::var_os("SAFEUSER");
        env::set_var("SAFEUSER", "tester");

        let request = CommitPushPrRequest {
            commit_message: Some("feat: add feature file".to_string()),
            pr_title: "Add feature file".to_string(),
            pr_body: "## Summary\n- add feature file".to_string(),
            branch_name_hint: "Add feature file".to_string(),
        };

        // when
        let report =
            handle_commit_push_pr_slash_command(&request, &repo, &runtime::NoopReporter).expect("commit-push-pr succeeds");
        let branch = run_command(&repo, "git", &["branch", "--show-current"]);
        let message = run_command(&repo, "git", &["log", "-1", "--pretty=%B"]);
        let gh_invocations = fs::read_to_string(&gh_log).expect("gh log should exist");

        // then
        assert!(report.contains("Result           created"));
        assert!(report.contains("URL              https://example.com/pr/123"));
        assert_eq!(branch.trim(), "tester/add-feature-file");
        assert_eq!(message.trim(), "feat: add feature file");
        assert!(gh_invocations.contains("pr create"));
        assert!(gh_invocations.contains("--base main"));

        if let Some(path) = previous_path {
            env::set_var("PATH", path);
        } else {
            env::remove_var("PATH");
        }
        if let Some(safeuser) = previous_safeuser {
            env::set_var("SAFEUSER", safeuser);
        } else {
            env::remove_var("SAFEUSER");
        }

        let _ = fs::remove_dir_all(repo);
        let _ = fs::remove_dir_all(remote);
        let _ = fs::remove_dir_all(fake_bin);
    }

    // ── New tests for SlashCategory, render_help_grouped, and try_user_command ──

    #[test]
    fn every_spec_has_category_and_predicate() {
        use super::{always_enabled, slash_command_specs};
        for spec in slash_command_specs() {
            // Calling is_enabled must not panic and must be callable
            let _enabled = (spec.is_enabled)();
            // Category must be a valid variant (pattern exhaustiveness ensures this at compile time,
            // but we also verify the order value is in [0..7])
            assert!(
                spec.category.order() <= 7,
                "spec '{}' has unexpected category order {}",
                spec.name,
                spec.category.order()
            );
            // Ensure always_enabled is wired up (at least it compiles and returns true)
            assert!(always_enabled());
        }
    }

    #[test]
    fn render_help_grouped_contains_category_labels() {
        use super::render_help_grouped;
        let output = render_help_grouped();
        // Must include at least some category labels
        assert!(
            output.contains("Session")
                || output.contains("Git")
                || output.contains("Analysis"),
            "render_help_grouped output missing expected category labels:\n{output}"
        );
        // Must not include hidden items (none are hidden by default, so all 36 should appear)
        assert!(output.contains("/help"));
        assert!(output.contains("/version"));
    }

    #[test]
    fn try_user_command_returns_none_when_unknown() {
        use super::try_user_command;
        use crate::user_commands::UserCommandRegistry;
        let registry = UserCommandRegistry::new();
        let cwd = Path::new("/tmp");
        assert!(try_user_command("/nonexistent", &registry, cwd).is_none());
    }

    #[test]
    fn try_user_command_expands_with_args() {
        use super::{try_user_command, ExpandedUserCommand};
        use crate::user_commands::UserCommandRegistry;

        let root = {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time ok")
                .as_nanos();
            std::env::temp_dir().join(format!("lib-try-user-cmd-{nanos}"))
        };
        let cmd_dir = root.join(".elai").join("commands");
        fs::create_dir_all(&cmd_dir).expect("create cmd dir");
        fs::write(cmd_dir.join("greet.md"), "Hello $ARGUMENTS!").expect("write greet.md");

        let registry = UserCommandRegistry::discover(&root).expect("discover");
        let cwd = Path::new("/my/project");
        let result = try_user_command("/greet world", &registry, cwd)
            .expect("greet command should match");

        assert_eq!(
            result,
            ExpandedUserCommand {
                command_name: "greet".to_string(),
                expanded_prompt: "Hello world!".to_string(),
                argument_hint: None,
            }
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_help_grouped_with_no_registry_matches_default() {
        use super::{render_help_grouped, render_help_grouped_with};
        assert_eq!(render_help_grouped_with(None), render_help_grouped());
    }

    #[test]
    fn render_help_grouped_with_registry_includes_custom_section() {
        use super::render_help_grouped_with;
        use crate::user_commands::UserCommandRegistry;

        // Build registry via discover on a tmpdir with two command files.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time ok")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lib-help-custom-{nanos}"));
        let cmd_dir = root.join(".elai").join("commands");
        fs::create_dir_all(&cmd_dir).expect("create cmd dir");
        fs::write(
            cmd_dir.join("alpha.md"),
            "---\ndescription: Alpha command\n---\ndo alpha $ARGUMENTS",
        )
        .expect("write alpha.md");
        fs::write(
            cmd_dir.join("beta.md"),
            "---\ndescription: Beta command\nargument-hint: [name]\n---\ndo beta $ARGUMENTS",
        )
        .expect("write beta.md");

        let registry = UserCommandRegistry::discover(&root).expect("discover");
        let output = render_help_grouped_with(Some(&registry));

        assert!(
            output.contains("Custom"),
            "output should contain 'Custom' section label"
        );
        assert!(output.contains("alpha"), "output should contain 'alpha'");
        assert!(output.contains("beta"), "output should contain 'beta'");
        assert!(
            output.contains("Alpha command"),
            "output should contain 'Alpha command' description"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
