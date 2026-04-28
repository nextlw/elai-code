# 🚀 Elai GenAI — Rust Implementation

A high-performance Rust rewrite of the Elai GenAI CLI agent harness. Built for speed, safety, and native tool execution.

> Formerly known as *Elai Code*. Rebranded to **Elai GenAI** with new features including Strict Write Discipline (SWD), retrieval-augmented context (`code_index`), multi-method authentication, and TUI enhancements.

**Current version:** `0.6.5`

## Quick Start

```bash
# Build
cd rust/
cargo build --release

# (First time) authenticate
./target/release/elai login                # opens an OAuth picker overlay
# or, non-interactive:
./target/release/elai login --import-claude-code   # reuses ~/.claude/credentials

# (First time) bootstrap the project (ELAI.md + RAG index + watcher)
./target/release/elai init

# Interactive TUI REPL
./target/release/elai

# One-shot prompt
./target/release/elai prompt "explain this codebase"

# Headless send (no TUI, streams to stdout)
./target/release/elai send "summarize the recent commits"

# Specific model
./target/release/elai --model sonnet prompt "fix the bug in main.rs"

# Strict Write Discipline (transactional writes)
./target/release/elai --swd full
```

## Configuration

Elai accepts credentials from several sources (first match wins):

1. **`ANTHROPIC_API_KEY`** environment variable (`sk-ant-…`).
2. **`ANTHROPIC_AUTH_TOKEN`** environment variable (Bearer token, e.g. proxy).
3. **macOS Keychain** entry `elai-credentials` (set via `elai login`).
4. `~/.elai/credentials.json` (mode `0600`).
5. Auto-import from Claude Code (`~/.claude/.credentials.json` or its Keychain).

Optional:

```bash
export ANTHROPIC_BASE_URL="https://your-proxy.example.com"   # custom gateway
export ELAI_CONFIG_HOME="/custom/.elai"                      # override config dir
export ELAI_SKIP_UPDATE=1                                     # disable bg update check
export ELAI_UPDATE_CHECK_INTERVAL_SECS=3600                   # default: 1 h
```

### Theme overrides

The TUI theme is resolved with this precedence:

1. Environment variables for the current process.
2. Persistent user config in `~/.elai/config.json`.
3. Defaults compiled into the CLI.

The secondary/meta text color defaults to ANSI grayscale `248`:

```bash
# Temporary override for one shell/session.
export ELAI_TEXT_SECONDARY_INTENSITY=240

# Runtime TUI command: applies immediately and persists to ~/.elai/config.json.
/theme gray 248
```

`text_secondary` uses the ANSI 256 grayscale ramp `232..=255`: lower values are
darker and higher values are lighter. Quick reference:

- `232` = almost black
- `240` = dark gray
- `244` = medium gray
- `248` = medium-light gray (default)
- `252` = light gray
- `255` = almost white

Values outside `232..=255` fall back to the default `248` without breaking
rendering. Advanced theme tokens can also be overridden with `ELAI_THEME_*`
environment variables or the `theme` object in `~/.elai/config.json`; color
values accept ANSI names (`white`, `dark_grey`), ANSI indexes (`0..=255`), or
hex RGB (`#RRGGBB`).

### Authentication methods

`elai login` opens an interactive picker, but every method has a flag for non-interactive / CI use:

| Flag | Method |
|------|--------|
| *(no flag)* | Interactive picker overlay |
| `--claudeai` | OAuth via claude.ai (Pro / Max / Team / Enterprise subscriber) |
| `--console` | OAuth via Anthropic Console (creates an API key) |
| `--sso` | OAuth via SSO (uses `login_method=sso`) |
| `--api-key` | Paste an `sk-ant-…` API key (`--stdin` to pipe) |
| `--token` | Paste an `ANTHROPIC_AUTH_TOKEN` (Bearer; `--stdin` to pipe) |
| `--use-bedrock` | Switch to AWS Bedrock (sets `CLAUDE_CODE_USE_BEDROCK=1`) |
| `--use-vertex` | Switch to Google Vertex AI (sets `CLAUDE_CODE_USE_VERTEX=1`) |
| `--use-foundry` | Switch to Azure Foundry (sets `CLAUDE_CODE_USE_FOUNDRY=1`) |
| `--import-claude-code` | Import credentials from Claude Code without interaction |
| `--legacy-elai` | Use the deprecated elai.dev OAuth flow |

Inspect / list:

```bash
elai auth status [--json]
elai auth list
elai logout
```

## Features

| Feature | Status |
|---------|--------|
| API + SSE streaming | ✅ |
| Multi-method authentication (API key, OAuth, Bearer, Bedrock/Vertex/Foundry, import-claude-code) | ✅ |
| Interactive TUI REPL (ratatui + crossterm) | ✅ |
| Tool system (bash, read, write, edit, grep, glob, notebook, agent, todo, …) | ✅ |
| Web tools (search, fetch) | ✅ |
| Sub-agent orchestration (`Agent` tool) | ✅ |
| ELAI.md / project memory | ✅ |
| Config file hierarchy (`.elai.json`, `.elai/settings.local.json`) | ✅ |
| Permission system (`read-only` / `workspace-write` / `danger-full-access`) | ✅ |
| MCP server lifecycle | ✅ |
| Session persistence + resume | ✅ |
| Extended thinking (thinking blocks) | ✅ |
| Cost tracking + usage display + `/budget` limiter | ✅ |
| Git integration (`/diff`, `/branch`, `/worktree`) | ✅ |
| Markdown terminal rendering (ANSI) | ✅ |
| Headless / agent mode (`send`, `chat`, `model`, `reply`, `status`) | ✅ |
| **RAG / code index (`code_index` crate, BGE-small + sqlite-vec)** | ✅ |
| **`@file` mention picker in the TUI** | ✅ |
| **Plugin system (tarball install + `/plugin` command)** | ✅ |
| **Slash command categories + Ctrl+K palette** | ✅ |
| **Strict Write Discipline (SWD)** | ✅ |
| **TUI-safe progress reporting (`TaskProgressReporter`)** | ✅ |
| **Background update check + Cloudflare Worker installer** | ✅ |
| Skills registry | ✅ |
| Hooks (PreToolUse / PostToolUse) | 🔧 Config only |

## CLI

```
elai [OPTIONS] [COMMAND]

Options:
  --model MODEL                  Set the model (alias or full ID; default: gpt-4o-mini)
  --permission-mode MODE         read-only | workspace-write | danger-full-access
                                 (default: danger-full-access)
  --config PATH                  Override path to the merged config file
  --output-format FORMAT         text | json | ndjson (default: text)
  --swd LEVEL                    Strict Write Discipline: off | partial | full (default: partial)
  --no-tui                       Disable TUI; use plain text REPL
  --yes                          Assume yes to every confirmation (CI / agent mode)
  --no                           Assume no to every confirmation
  --version, -V                  Print version

Commands:
  prompt <text…>                 One-shot prompt (non-interactive)
  send <message…>                Headless send (streaming) — supports --wait, --json, --stdin
  chat show [--last N] [--json]  View recent chat history
  reply <answer…>                Reply to a pending model question
  status [--json]                Print current session status
  model get | set <model>        Show or set the active model
  login [METHOD]                 Authenticate (see Authentication methods above)
  logout                         Clear stored credentials
  auth status [--json] | list    Inspect or enumerate auth methods
  init [INDEX FLAGS]             Initialize project (ELAI.md + RAG index + watcher)
  dump-manifests                 Read upstream TS sources and print extracted counts
  bootstrap-plan                 Print the current bootstrap-phase skeleton
```

### `elai init` flags

```
--backend <sqlite|qdrant>          Vector store backend (default: sqlite)
--qdrant-url <URL>                 Qdrant URL (when --backend qdrant)
--embed-provider <local|ollama|jina|openai|voyage>
                                   Embedding provider (default: local — fastembed BGE-small)
--embed-model <name>               Override the default model for the chosen provider
--ollama-url <URL>                 Ollama URL (when --embed-provider ollama)
--no-watcher                       Skip starting the background filesystem watcher
--no-index                         Skip indexing (only create files + ELAI.md)
--reindex                          Drop the existing index and rebuild from scratch
```

The default backend uses [`fastembed`](https://github.com/Anush008/fastembed-rs) with the BGE-small model and `sqlite-vec` for storage. Targets without a prebuilt `ort` (e.g. `aarch64-unknown-linux-musl`) must be built with `--no-default-features` — the `local` provider is then unavailable, but `ollama`/`http` providers still work.

## Slash Commands (REPL)

Commands are grouped by category. The full registry lives in `crates/commands/src/lib.rs::SLASH_COMMAND_SPECS`.

### Session

| Command | Description |
|---------|-------------|
| `/help` | Show available commands grouped by category |
| `/status` | Current session status (model, tokens, cost) |
| `/clear [--confirm]` | Start a fresh local session |
| `/compact` | Compact local session history |
| `/cost` | Cumulative token usage |
| `/resume <session-path>` | Load a saved session into the REPL |
| `/export [file]` | Export the conversation |

### Behavior

| Command | Description |
|---------|-------------|
| `/model [model]` | Show or switch the active model |
| `/permissions [mode]` | Show or switch permission mode |
| `/tools [why]` | Inspect tool selection for the current session |
| `/budget [tokens] [usd] \| off` | Set token / USD budget limiter |
| `/cache [clear\|stats]` | Manage the response cache |
| `/providers [--verbose]` | Provider usage dashboard |
| `/swd [off\|partial\|full]` | Show or change SWD level |
| `/theme gray <232-255>` | Adjust secondary/meta gray and persist it to `~/.elai/config.json` |

### Project

| Command | Description |
|---------|-------------|
| `/init` | Create a starter ELAI.md for this repo |
| `/memory` | Inspect loaded ELAI.md / instruction memory |
| `/config [env\|hooks\|model\|plugins]` | Inspect merged config sections |
| `/verify` | Verify codebase files against memory entries |

### Git

| Command | Description |
|---------|-------------|
| `/diff` | Show git diff for current changes |
| `/branch [list\|create <n>\|switch <n>]` | List, create, or switch branches |
| `/worktree [list\|add <path> [branch]\|remove <path>\|prune]` | Manage worktrees |
| `/commit` | Generate a commit message and create a commit *(AI-driven; preview)* |
| `/commit-push-pr [context]` | Commit, push the branch, and open a PR *(AI-driven; preview)* |
| `/pr [context]` | Draft or create a pull request *(AI-driven; preview)* |
| `/issue [context]` | Draft or create a GitHub issue *(AI-driven; preview)* |

### Analysis

| Command | Description |
|---------|-------------|
| `/bughunter [scope]` | Inspect the codebase for likely bugs *(AI-driven; preview)* |
| `/ultraplan [task]` | Deep planning prompt with multi-step reasoning *(preview)* |
| `/teleport <symbol-or-path>` | Jump to a file or symbol *(preview)* |
| `/debug-tool-call` | Replay the last tool call with debug details |

### System

| Command | Description |
|---------|-------------|
| `/version` | Show CLI version and build information |
| `/update` | Check for and install the latest release |

### Plugins

| Command | Description |
|---------|-------------|
| `/plugin [list\|install <path>\|enable\|disable\|uninstall\|update]` (aliases: `/plugins`, `/marketplace`) | Manage plugins |
| `/agents` | List configured agents |
| `/skills` | List available skills |
| `/dream [--force]` | Compress old memory entries into a summary |
| `/stats [--days N] [--by-model] [--by-project]` | Token usage and cost statistics |
| `/session [list\|switch <id>]` | List or switch managed sessions |

### Custom

User-defined `.md` commands placed under `.elai/commands/` are auto-discovered and shown under **Custom** in `/help` and the Ctrl+K palette.

**Keyboard shortcuts:** `F2`=model · `F3`=permissions · `F4`=sessions · `Ctrl+K`=palette · `@`=file mention picker

## RAG / Code Index

`crates/code_index/` provides retrieval-augmented context. After `elai init`, the project is walked, chunked (tree-sitter for Rust / TS / JS / Python / Go / Markdown / generic), embedded (BGE-small via `fastembed` by default), and stored in a `sqlite-vec` (or Qdrant) index under `.elai/index.db`.

- A background watcher reindexes changed files (disable with `--no-watcher`).
- Use `@<path>` inside the TUI to mention a specific file — the matched files are appended to the system prompt as `## Mentioned files`.
- Run `elai init --reindex` to drop and rebuild the index.

## 🛡️ Strict Write Discipline (SWD)

SWD is a transactional filesystem write engine that adds safety and auditability to every file modification the agent makes.

### Levels

| Level | Description |
|-------|-------------|
| `off` | Normal tool execution — no write interception |
| `partial` *(default)* | Every write tool call is wrapped with SHA-256 before/after snapshots; failures trigger automatic rollback; all operations are logged to `.elai/swd.log` |
| `full` | Write tools (`write_file`, `edit_file`, `NotebookEdit`) are **blocked**; the model must emit `[FILE_ACTION]` blocks in its text output, which are executed transactionally with hash verification and full rollback on any failure |

### Usage

```bash
elai --swd full
elai --swd partial
elai --swd off

# REPL command (cycles through levels without argument)
/swd
/swd full
/swd off
```

### Full Mode — `[FILE_ACTION]` Blocks

In full mode, the model emits structured blocks instead of calling write tools:

```
[FILE_ACTION:Write]
path: relative/path/to/file.rs
content_hash: <sha256-hex-of-exact-content>
---
<exact file content here>
[/FILE_ACTION]

[FILE_ACTION:Delete]
path: relative/path/to/file.rs
---
[/FILE_ACTION]
```

The engine snapshots all target files, executes the actions sequentially, verifies content hashes, and rolls back **all** actions if any single one fails.

### Audit Log

All SWD transactions are appended to `.elai/swd.log` as JSON-lines:

```json
{"ts":1711843200000,"tool":"write_file","path":"src/main.rs","outcome":"Verified","before":"abc123…","after":"def456…"}
```

### TUI Integration

- SWD level is shown in the status-bar footer: `SWD:partial`.
- A dedicated `SwdLogEntry` chat widget shows per-file results: ✓ Verified · · Noop · ~ Drift · ✗ Failed · ↩ Rolled back.

## TUI-safe Progress Reporting

Long-running commands (`/init`, `/verify`, `/dream`, `/commit-push-pr`, plugin install, BGE download, …) cannot use `eprintln!`/`println!`/`indicatif` while the ratatui alternate screen is active — ANSI control codes corrupt the rendering.

The `runtime::tasks::progress` module exposes:

- `TaskProgressReporter` — implements the `runtime::ProgressReporter` trait and routes updates through a `ProgressSink`.
- `with_task` / `with_task_default` — register a task in the process-wide `TaskRegistry`, hand a reporter to the closure, and finalise the task on return.
- `set_default_sink(Arc<dyn ProgressSink>)` — install the global sink at process startup.

Built-in sinks in `runtime::tasks::sinks`:

| Sink | When to use |
|---|---|
| `LiveStderrSink` | CLI on a TTY — repaints with `\r\x1b[2K` (throttled to 80 ms, respects `$COLUMNS`) |
| `PlainStderrSink` | CLI piping / CI — emits a new line at every 5 % delta |
| `NoopSink` | Quiet / batch mode |
| `CollectingSink` | Tests or deferred rendering |
| `ChannelSink` (in `elai-cli/src/tui_sink.rs`) | TUI — forwards updates as `TuiMsg::TaskProgress{,End}` |

See `docs/progress-pattern.md` for the full convention and code samples.

## Update / Distribution

```bash
elai self-update           # in-place upgrade to the latest release
/update                    # same, from inside the TUI
```

The TUI also runs a non-blocking background check every `ELAI_UPDATE_CHECK_INTERVAL_SECS` (default `3600`) and surfaces a single `SystemNote` per new version. Disable with `ELAI_SKIP_UPDATE=1` (and automatically disabled when `CI=1`).

The shell installer is served from a Cloudflare Worker at `get.nexcode.live`; user-agent detection routes `sh` clients to `/sh` and PowerShell clients to `/ps`.

## Model Aliases

Short names resolve to the latest model versions:

| Alias | Resolves To |
|-------|------------|
| `opus` | `claude-opus-4-6` |
| `sonnet` | `claude-sonnet-4-6` |
| `haiku` | `claude-haiku-4-5-20251001` |

## Supported Model Pricing

| Model | Input $/M | Output $/M |
|-------|-----------|------------|
| Claude Opus | 15.00 | 75.00 |
| Claude Sonnet | 3.00 | 15.00 |
| Claude Haiku | 0.25 | 1.25 |
| GPT-4o-mini | 0.15 | 0.60 |

## Workspace Layout

```
rust/
├── Cargo.toml              # Workspace root (resolver = "2", 10 crates)
├── Cargo.lock
├── README.md / ELAI.md / CONTRIBUTING.md
├── docs/
│   └── progress-pattern.md # TUI-safe progress reporting convention
└── crates/
    ├── api/                # HTTP client, SSE stream parser, request/response types
    ├── code_index/         # 🆕 RAG: walker + tree-sitter chunkers + embedders + sqlite-vec / Qdrant
    ├── commands/           # Slash command registry, categories, providers, user commands
    ├── compat-harness/     # Extracts tool/prompt manifests from upstream TS source
    ├── elai-cli/           # Main CLI binary — TUI, REPL, args, SWD, init, verify, dream
    ├── lsp/                # LSP-types re-export (lsp-types crate workspace dep)
    ├── plugins/            # Plugin manager (tarball install, sandboxing, lifecycle)
    ├── runtime/            # Conversation runtime, sessions, config, MCP, OAuth, tasks, prompts
    ├── server/             # Headless / agent-mode HTTP surface
    └── tools/              # Built-in tool implementations
```

### Key crate responsibilities

- **api** — HTTP client, SSE stream parser, request/response types, auth (API key + OAuth bearer).
- **code_index** — RAG foundation. Traits `Embedder`, `VectorStore`. Impls: `SqliteVecStore`, `MemoryStore`, `LocalFastEmbedder` (BGE-small), `OllamaEmbedder`, `JinaEmbedder`, `OpenaiEmbedder`, `VoyageEmbedder`. Includes `Indexer`, `collect_facts`, `ProjectFacts`.
- **commands** — `SLASH_COMMAND_SPECS`, `SlashCategory`, `SlashCommand::parse`, `render_help_grouped`, `UserCommandRegistry` (custom `.md` commands), provider/stats helpers.
- **compat-harness** — Extracts tool/prompt manifests from upstream TS source.
- **runtime** — `ConversationRuntime` agentic loop · `ConfigLoader` hierarchy · `Session` persistence · permission policy · MCP client · system-prompt assembly · usage tracking · model pricing · OAuth (`OAuthAuthorizationRequest`, `OAuthTokenSet`, …) · `tasks::TaskRegistry` (process-wide singleton) · `tasks::progress` (`TaskProgressReporter`, `with_task[_default]`, `set_default_sink`) · `tasks::sinks` (`ProgressSink`, `LiveStderrSink`, `PlainStderrSink`, `ChannelSink`, …) · `oneshot::generate_elai_md_with` · `parse_mentions` / `read_mentioned_files`.
- **elai-cli** — TUI REPL, headless `send`/`chat`/`reply`, streaming display, tool-call rendering, CLI argument parsing, **SWD engine**, `tui_sink::ChannelSink`, init/verify/dream/diff/auth wiring.
- **plugins** — Plugin discovery, tarball install, sandboxing, manifest validation, lifecycle hooks.
- **server** — Headless HTTP surface for agent mode.
- **tools** — Tool specs + execution: Bash, ReadFile, WriteFile, EditFile, GlobSearch, GrepSearch, WebSearch, WebFetch, Agent, TodoWrite, NotebookEdit, Skill, ToolSearch, REPL runtimes.

## Stats

- **~25 K+ lines** of Rust
- **10 crates** in workspace
- **Binary name:** `elai`
- **Default model:** `gpt-4o-mini` (override with `--model` or `/model`)
- **Default permissions:** `danger-full-access`
- **Default SWD:** `partial`

## Verification

Run the full Rust verification set before opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
cargo test --workspace
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full development workflow.

## License

See repository root.
