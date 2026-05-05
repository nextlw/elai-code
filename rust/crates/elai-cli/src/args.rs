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

    /// Assume "yes" to all confirmations (non-interactive / agent mode)
    #[arg(long, global = true)]
    pub yes: bool,

    /// Assume "no" to all confirmations (non-interactive / agent mode)
    #[arg(long, global = true)]
    pub no: bool,

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
    /// Send a message directly without opening the TUI
    Send {
        /// Message to send. Use "-" or --stdin to read from stdin.
        message: Vec<String>,
        /// Wait for the full response before returning (default: streaming)
        #[arg(long)]
        wait: bool,
        /// Output response as JSON
        #[arg(long)]
        json: bool,
        /// Read message from stdin
        #[arg(long)]
        stdin: bool,
    },
    /// View chat history
    Chat {
        #[command(subcommand)]
        cmd: ChatCmd,
    },
    /// Manage the active model
    Model {
        #[command(subcommand)]
        cmd: ModelCmd,
    },
    /// Reply to a pending question from the model
    Reply {
        /// The answer to provide
        answer: Vec<String>,
        /// Read answer from stdin
        #[arg(long)]
        stdin: bool,
    },
    /// Show current session status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Initialize project: create .elai/, ELAI.md, and index code
    Init(InitArgs),
}

#[derive(Debug, Clone, clap::Args, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct InitArgs {
    /// Backend de armazenamento vetorial
    #[arg(long, value_enum, default_value_t = IndexBackend::Sqlite)]
    pub backend: IndexBackend,
    /// URL do Qdrant (apenas para --backend qdrant)
    #[arg(long)]
    pub qdrant_url: Option<String>,
    /// Provedor de embeddings
    #[arg(long, value_enum, default_value_t = EmbedProviderArg::Local)]
    pub embed_provider: EmbedProviderArg,
    /// Modelo de embedding (override do default por provider)
    #[arg(long)]
    pub embed_model: Option<String>,
    /// URL do Ollama (apenas para --embed-provider ollama)
    #[arg(long)]
    pub ollama_url: Option<String>,
    /// Não rodar watcher background após init
    #[arg(long)]
    pub no_watcher: bool,
    /// Pular indexação (apenas cria arquivos básicos + ELAI.md template)
    #[arg(long)]
    pub no_index: bool,
    /// Apaga índice existente e reindexa do zero
    #[arg(long)]
    pub reindex: bool,
    /// Sobe (ou inicia) um container Qdrant local via Docker
    #[arg(long)]
    pub start_qdrant: bool,
    /// Porta HTTP do Qdrant quando --start-qdrant estiver ativo
    #[arg(long, default_value_t = 6333)]
    pub qdrant_port: u16,
    /// Nome do container Docker do Qdrant quando --start-qdrant estiver ativo
    #[arg(long, default_value = "elai-qdrant")]
    pub qdrant_container: String,
}

impl Default for InitArgs {
    fn default() -> Self {
        Self {
            backend: IndexBackend::Sqlite,
            qdrant_url: None,
            embed_provider: EmbedProviderArg::Local,
            embed_model: None,
            ollama_url: None,
            no_watcher: false,
            no_index: false,
            reindex: false,
            start_qdrant: false,
            qdrant_port: 6333,
            qdrant_container: "elai-qdrant".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub enum IndexBackend {
    Sqlite,
    Qdrant,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub enum EmbedProviderArg {
    Local,
    Ollama,
    Jina,
    Openai,
    Voyage,
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum ChatCmd {
    /// Show recent chat history
    Show {
        /// Number of messages to show
        #[arg(long, default_value = "20")]
        last: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum ModelCmd {
    /// Show the currently active model
    Get,
    /// Set the active model
    Set {
        /// Model name or alias (e.g. opus, sonnet, claude-opus-4-6)
        model: String,
    },
}

#[derive(Debug, Clone, clap::Args, PartialEq, Eq)]
#[group(required = false, multiple = false, id = "method")]
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct LoginArgs {
    /// Login via Anthropic Console (creates an API key)
    #[arg(long, group = "method")]
    pub console: bool,
    /// Login via claude.ai (Pro/Max/Team/Enterprise subscriber OAuth)
    #[arg(long, group = "method")]
    pub claudeai: bool,
    /// Login via SSO (uses claude.ai flow with `login_method=sso`)
    #[arg(long, group = "method")]
    pub sso: bool,
    /// Pre-fill the e-mail on the OAuth login page (`login_hint`)
    #[arg(long)]
    pub email: Option<String>,
    /// Paste an Anthropic API key (sk-ant-...). Use --stdin to pipe; otherwise prompts securely.
    #[arg(long, group = "method")]
    pub api_key: bool,
    /// Paste an `ANTHROPIC_AUTH_TOKEN` (Bearer token). Use --stdin to pipe.
    #[arg(long, group = "method")]
    pub token: bool,
    /// Switch to AWS Bedrock (sets `CLAUDE_CODE_USE_BEDROCK=1` in shell rc; AWS creds via standard chain)
    #[arg(long, group = "method")]
    pub use_bedrock: bool,
    /// Switch to Google Vertex AI (sets `CLAUDE_CODE_USE_VERTEX=1`)
    #[arg(long, group = "method")]
    pub use_vertex: bool,
    /// Switch to Azure Foundry (sets `CLAUDE_CODE_USE_FOUNDRY=1`)
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
    /// Import credentials from Claude Code (~/.claude/credentials.json) without interaction
    #[arg(long, group = "method")]
    pub import_claude_code: bool,
    /// Login OAuth via Codex/OpenAI (`codex login`) and import credentials
    #[arg(long, group = "method")]
    pub codex_oauth: bool,
    /// Import Codex `ChatGPT` OAuth from ~/.codex/auth.json (or $`CODEX_HOME/auth.json`)
    #[arg(long, group = "method")]
    pub import_codex: bool,
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

    use super::{
        AuthCmd, ChatCmd, Cli, Command, EmbedProviderArg, IndexBackend, InitArgs, LoginArgs,
        ModelCmd, OutputFormat, PermissionMode,
    };

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
        assert_eq!(login.command, Some(Command::Login(LoginArgs::default())));

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

    #[test]
    fn parses_send_command() {
        let cli = Cli::parse_from(["elai-cli", "send", "hello", "world"]);
        assert_eq!(
            cli.command,
            Some(Command::Send {
                message: vec!["hello".into(), "world".into()],
                wait: false,
                json: false,
                stdin: false,
            })
        );
    }

    #[test]
    fn parses_send_with_json_flag() {
        let cli = Cli::parse_from(["elai-cli", "send", "--json", "explain this"]);
        assert_eq!(
            cli.command,
            Some(Command::Send {
                message: vec!["explain this".into()],
                wait: false,
                json: true,
                stdin: false,
            })
        );
    }

    #[test]
    fn parses_login_import_claude_code() {
        let cli = Cli::parse_from(["elai-cli", "login", "--import-claude-code"]);
        assert_eq!(
            cli.command,
            Some(Command::Login(LoginArgs {
                import_claude_code: true,
                ..LoginArgs::default()
            }))
        );
    }

    #[test]
    fn parses_login_import_codex() {
        let cli = Cli::parse_from(["elai-cli", "login", "--import-codex"]);
        assert_eq!(
            cli.command,
            Some(Command::Login(LoginArgs {
                import_codex: true,
                ..LoginArgs::default()
            }))
        );
    }

    #[test]
    fn parses_login_codex_oauth() {
        let cli = Cli::parse_from(["elai-cli", "login", "--codex-oauth"]);
        assert_eq!(
            cli.command,
            Some(Command::Login(LoginArgs {
                codex_oauth: true,
                ..LoginArgs::default()
            }))
        );
    }

    #[test]
    fn parses_model_set() {
        let cli = Cli::parse_from(["elai-cli", "model", "set", "claude-opus-4-6"]);
        assert_eq!(
            cli.command,
            Some(Command::Model {
                cmd: ModelCmd::Set {
                    model: "claude-opus-4-6".into()
                }
            })
        );
    }

    #[test]
    fn parses_model_get() {
        let cli = Cli::parse_from(["elai-cli", "model", "get"]);
        assert_eq!(cli.command, Some(Command::Model { cmd: ModelCmd::Get }));
    }

    #[test]
    fn parses_status_json() {
        let cli = Cli::parse_from(["elai-cli", "status", "--json"]);
        assert_eq!(cli.command, Some(Command::Status { json: true }));
    }

    #[test]
    fn parses_yes_flag_globally() {
        let cli = Cli::parse_from(["elai-cli", "--yes", "send", "hello"]);
        assert!(cli.yes);
    }

    #[test]
    fn parses_chat_show() {
        let cli = Cli::parse_from(["elai-cli", "chat", "show", "--last", "5"]);
        assert_eq!(
            cli.command,
            Some(Command::Chat {
                cmd: ChatCmd::Show {
                    last: 5,
                    json: false
                }
            })
        );
    }

    #[test]
    fn parses_init_with_defaults() {
        let cli = Cli::parse_from(["elai", "init"]);
        assert_eq!(cli.command, Some(Command::Init(InitArgs::default())));
    }

    #[test]
    fn parses_init_with_ollama() {
        let cli = Cli::parse_from([
            "elai",
            "init",
            "--embed-provider",
            "ollama",
            "--ollama-url",
            "http://localhost:11434",
        ]);
        assert_eq!(
            cli.command,
            Some(Command::Init(InitArgs {
                embed_provider: EmbedProviderArg::Ollama,
                ollama_url: Some("http://localhost:11434".to_string()),
                ..InitArgs::default()
            }))
        );
    }

    #[test]
    fn parses_init_with_no_index_flag() {
        let cli = Cli::parse_from(["elai", "init", "--no-index"]);
        assert_eq!(
            cli.command,
            Some(Command::Init(InitArgs {
                no_index: true,
                ..InitArgs::default()
            }))
        );
    }

    #[test]
    fn parses_init_with_backend_sqlite() {
        let cli = Cli::parse_from(["elai", "init", "--backend", "sqlite"]);
        assert_eq!(
            cli.command,
            Some(Command::Init(InitArgs {
                backend: IndexBackend::Sqlite,
                ..InitArgs::default()
            }))
        );
    }

    #[test]
    fn parses_init_with_start_qdrant_flags() {
        let cli = Cli::parse_from([
            "elai",
            "init",
            "--start-qdrant",
            "--qdrant-port",
            "7333",
            "--qdrant-container",
            "meu-qdrant",
        ]);
        assert_eq!(
            cli.command,
            Some(Command::Init(InitArgs {
                start_qdrant: true,
                qdrant_port: 7333,
                qdrant_container: "meu-qdrant".to_string(),
                ..InitArgs::default()
            }))
        );
    }
}
