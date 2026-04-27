use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Parser, PartialEq, Eq)]
#[command(name = "elai-cli", version, about = "Elai Code CLI")]
pub struct Cli {
    #[arg(long, default_value = "gpt-4o-mini")]
    pub model: String,

    #[arg(long, value_enum, default_value_t = PermissionMode::DangerFullAccess)]
    pub permission_mode: PermissionMode,

    #[arg(long)]
    pub config: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub output_format: OutputFormat,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Read upstream TS sources and print extracted counts
    DumpManifests,
    /// Print the current bootstrap phase skeleton
    BootstrapPlan,
    /// Authenticate with Anthropic or a third-party provider
    Login(LoginArgs),
    /// Clear saved authentication credentials
    Logout,
    /// Run a non-interactive prompt and exit
    Prompt { prompt: Vec<String> },
    /// Show or list authentication methods
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },
}

#[derive(Debug, Clone, clap::Args, PartialEq, Eq)]
#[group(required = false, multiple = false, id = "method")]
pub struct LoginArgs {
    /// Login via Anthropic Console (creates an API key)
    #[arg(long, group = "method")]
    pub console: bool,
    /// Login via claude.ai (Pro/Max/Team/Enterprise subscriber OAuth)
    #[arg(long, group = "method")]
    pub claudeai: bool,
    /// Login via SSO (uses claude.ai flow with login_method=sso)
    #[arg(long, group = "method")]
    pub sso: bool,
    /// Pre-fill the e-mail on the OAuth login page (login_hint)
    #[arg(long)]
    pub email: Option<String>,
    /// Paste an Anthropic API key (sk-ant-...). Use --stdin to pipe; otherwise prompts securely.
    #[arg(long, group = "method")]
    pub api_key: bool,
    /// Paste an ANTHROPIC_AUTH_TOKEN (Bearer token). Use --stdin to pipe.
    #[arg(long, group = "method")]
    pub token: bool,
    /// Switch to AWS Bedrock (sets CLAUDE_CODE_USE_BEDROCK=1 in shell rc; AWS creds via standard chain)
    #[arg(long, group = "method")]
    pub use_bedrock: bool,
    /// Switch to Google Vertex AI (sets CLAUDE_CODE_USE_VERTEX=1)
    #[arg(long, group = "method")]
    pub use_vertex: bool,
    /// Switch to Azure Foundry (sets CLAUDE_CODE_USE_FOUNDRY=1)
    #[arg(long, group = "method")]
    pub use_foundry: bool,
    /// Print the OAuth URL but don't open a browser (CI / remote shells)
    #[arg(long)]
    pub no_browser: bool,
    /// Read the secret value from stdin (for --api-key and --token)
    #[arg(long)]
    pub stdin: bool,
    /// Use the legacy elai.dev OAuth flow (deprecated)
    #[arg(long)]
    pub legacy_elai: bool,
}

impl Default for LoginArgs {
    fn default() -> Self {
        Self {
            console: false,
            claudeai: false,
            sso: false,
            email: None,
            api_key: false,
            token: false,
            use_bedrock: false,
            use_vertex: false,
            use_foundry: false,
            no_browser: false,
            stdin: false,
            legacy_elai: false,
        }
    }
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum AuthCmd {
    /// Show the active authentication method, expiry, and scopes
    Status {
        #[arg(long)]
        json: bool,
    },
    /// List all available login methods
    List,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum PermissionMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    Ndjson,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{AuthCmd, Cli, Command, LoginArgs, OutputFormat, PermissionMode};

    #[test]
    fn parses_requested_flags() {
        let cli = Cli::parse_from([
            "elai-cli",
            "--model",
            "claude-haiku-4-5-20251001",
            "--permission-mode",
            "read-only",
            "--config",
            "/tmp/config.toml",
            "--output-format",
            "ndjson",
            "prompt",
            "hello",
            "world",
        ]);

        assert_eq!(cli.model, "claude-haiku-4-5-20251001");
        assert_eq!(cli.permission_mode, PermissionMode::ReadOnly);
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/tmp/config.toml"))
        );
        assert_eq!(cli.output_format, OutputFormat::Ndjson);
        assert_eq!(
            cli.command,
            Some(Command::Prompt {
                prompt: vec!["hello".into(), "world".into()]
            })
        );
    }

    #[test]
    fn parses_login_and_logout_commands() {
        let login = Cli::parse_from(["elai-cli", "login"]);
        assert_eq!(
            login.command,
            Some(Command::Login(LoginArgs::default()))
        );

        let logout = Cli::parse_from(["elai-cli", "logout"]);
        assert_eq!(logout.command, Some(Command::Logout));
    }

    #[test]
    fn defaults_to_danger_full_access_permission_mode() {
        let cli = Cli::parse_from(["elai-cli"]);
        assert_eq!(cli.permission_mode, PermissionMode::DangerFullAccess);
    }

    #[test]
    fn login_method_flags_are_mutually_exclusive() {
        let result = Cli::try_parse_from(["elai-cli", "login", "--console", "--api-key"]);
        assert!(result.is_err(), "mutually exclusive flags should fail");
    }

    #[test]
    fn login_with_console_flag_parses() {
        let cli = Cli::parse_from(["elai-cli", "login", "--console"]);
        assert_eq!(
            cli.command,
            Some(Command::Login(LoginArgs {
                console: true,
                ..LoginArgs::default()
            }))
        );
    }

    #[test]
    fn login_with_email_implies_login_hint_only() {
        // --email without a method flag is still valid
        let cli = Cli::parse_from(["elai-cli", "login", "--email", "user@example.com"]);
        assert_eq!(
            cli.command,
            Some(Command::Login(LoginArgs {
                email: Some("user@example.com".into()),
                ..LoginArgs::default()
            }))
        );
    }

    #[test]
    fn auth_status_subcommand_parses() {
        let cli = Cli::parse_from(["elai-cli", "auth", "status", "--json"]);
        assert_eq!(
            cli.command,
            Some(Command::Auth {
                cmd: AuthCmd::Status { json: true }
            })
        );
    }
}
