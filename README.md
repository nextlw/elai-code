# Elai Code

<p align="center">
  <img src="assets/logo.svg" alt="Elai Code" width="720" />
</p>

<p align="center">
  <strong>A high-performance CLI agent harness built in Rust by <a href="https://nexcode.live">Nexcode</a></strong>
</p>

<p align="center">
  <a href="https://github.com/nextlw/elai-code/releases"><img src="https://img.shields.io/github/v/release/nextlw/elai-code?style=for-the-badge&color=orange" alt="Release" /></a>
  <a href="https://github.com/nextlw/elai-code/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=for-the-badge" alt="License" /></a>
  <img src="https://img.shields.io/badge/built_with-Rust-orange?style=for-the-badge&logo=rust" alt="Rust" />
</p>

## Install

**macOS / Linux**

```sh
curl -fsSL https://get.nexcode.live | sh
```

**Windows** (PowerShell)

```powershell
irm https://get.nexcode.live/ps | iex
```

**Windows** (CMD)

```cmd
powershell -Command "irm https://get.nexcode.live/ps | iex"
```

All three commands download the latest binary from the latest GitHub release, install it, and add it to your PATH. The `get.nexcode.live` endpoint is a Cloudflare Worker that detects the client User-Agent and serves the matching install script (`install.sh` for shells, `install.ps1` for PowerShell). You can force a script explicitly:

```sh
curl -fsSL https://get.nexcode.live/sh | sh    # always serve install.sh
irm https://get.nexcode.live/ps | iex          # always serve install.ps1
```

After installing, open a new terminal and run:

```sh
elai
```

---

Elai Code is a modular, memory-safe agent harness that lets AI models safely interact with your filesystem, codebase, web resources, and remote agents — with transactional write guarantees, real-time cost tracking, and a polished terminal UI.

## What's New — v0.7.8

- feat(tui): add ultrathink feature to enhance message processing
- feat(tui): enable mouse capture in TUI for improved user interaction
- feat(tui): implement message queue for pending user inputs

---

## Features

### Strict Write Discipline (SWD)

A transactional filesystem write engine with three operating levels:

| Level                 | Behavior                                                                                                                                    |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `off`                 | Normal tool execution — no interception                                                                                                     |
| `partial` _(default)_ | Wraps every write with SHA-256 snapshots, automatic rollback on failure, and a JSON-lines audit log                                         |
| `full`                | Blocks all write tools; the model emits structured `[FILE_ACTION]` blocks executed transactionally with hash verification and full rollback |

```
--swd off|partial|full      CLI flag
/swd [off|partial|full]     REPL command (cycles levels when called without argument)
```

### Multi-Provider Support

| Provider          | Models                                                                                             | Notes                                                  |
| ----------------- | -------------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| Anthropic         | `claude-opus-4-6`, `claude-sonnet-4-6`, `claude-haiku-4-5` (and aliases `opus`, `sonnet`, `haiku`) | OAuth (Pro/Max), Console, SSO, API key, Bearer token   |
| OpenAI            | `gpt-4o`, `gpt-4o-mini`, `gpt-5*`, `o1*`, `o3*`, `o4*`, `chatgpt-*`, `ft:*`                        | API key via env or AuthPicker (`AuthMethod::OpenAiApiKey`) |
| xAI (Grok)        | `grok-3`, `grok-3-mini`, `grok-2`                                                                  | API key via `XAI_API_KEY`                              |
| Ollama (local)    | Any model loaded — use `ollama:NAME` (e.g. `ollama:llama3.1`, `ollama:qwen2.5-coder`)              | OpenAI-compatible API at `:11434/v1`, no auth needed   |
| LM Studio (local) | Any model loaded — use `lmstudio:NAME` (e.g. `lmstudio:qwen2.5-coder`)                             | OpenAI-compatible API at `:1234/v1`, no auth needed    |
| AWS Bedrock       | Claude models served by Bedrock                                                                    | `CLAUDE_CODE_USE_BEDROCK=1`, AWS standard creds        |
| Google Vertex     | Claude models served by Vertex AI                                                                  | `CLAUDE_CODE_USE_VERTEX=1`                             |
| Azure Foundry     | Claude models served by Azure                                                                      | `CLAUDE_CODE_USE_FOUNDRY=1`                            |

### MCP Integration

Full [Model Context Protocol](https://modelcontextprotocol.io/) support with `stdio`, `SDK`, and managed proxy transports — extend the tool ecosystem without modifying the harness.

### Tool Catalog

Declarative TOML-based tool definitions, MatcherPattern wildcards, a 5-stage selection pipeline, and per-session rate limiting and budget caps.

### Interactive TUI

- Animated braille spinner for thinking states
- Proper word-wrap for long paths, URLs, and JSON blobs
- Per-file SWD status widget with color-coded icons (✓ verified · ✗ failed · ↩ rolled back · ~ drift)
- Real-time USD cost display in the status bar
- Markdown rendering with syntax highlighting

### Session & Permissions

- Persistent sessions with resumption and compaction
- Permission modes: `read-only`, `workspace-write`, `danger-full-access`
- Budget caps with hard limits and live tracking

---

## Quickstart

### 1. Install

See the [Install](#install) section above for one-liners. Or build from source:

```bash
git clone https://github.com/nextlw/elai-code.git
cd elai-code/rust
cargo build --release
# binary: rust/target/release/elai
```

### 2. Pick a provider

**Anthropic (cloud):**
```bash
export ANTHROPIC_API_KEY=your_key_here
```

**OpenAI (cloud):**
```bash
export OPENAI_API_KEY=your_key_here
# Or persist via the TUI AuthPicker — see "Authentication" below.
```

**xAI Grok (cloud):**
```bash
export XAI_API_KEY=your_key_here
```

**Ollama (local):**
```bash
# Start Ollama (default port 11434):
ollama serve &
ollama pull llama3.1

# Tell Elai where it lives (only needed if non-default host/port):
export OLLAMA_BASE_URL=http://localhost:11434/v1

# Run a specific model with the explicit prefix:
elai --model ollama:llama3.1
```

**LM Studio (local):**
```bash
# Inside LM Studio: Developer → Start Server (default port 1234).
export LMSTUDIO_BASE_URL=http://localhost:1234/v1

# Run any loaded model — use the model id LM Studio reports:
elai --model lmstudio:qwen2.5-coder-7b-instruct
```

### 3. Run

```bash
elai
```

Switch provider at runtime:

```bash
elai --model gpt-4o-mini
elai --model ollama:llama3.1
elai --model lmstudio:qwen2.5-coder-7b-instruct
elai --model grok-3
```

> **Local model defaults:** providers locais usam `ELAI_LOCAL_MAX_COMPLETION_TOKENS` (default `4096`) como limite de saída. Sobrescreva se o modelo suportar contextos maiores: `export ELAI_LOCAL_MAX_COMPLETION_TOKENS=8192`.

> **Default model selection:** sem `--model` e sem credenciais cloud, mas com `OLLAMA_BASE_URL` setado, o Elai usa `ELAI_DEFAULT_OLLAMA_MODEL` (default `ollama:llama3.1`). Para LM Studio: `ELAI_DEFAULT_LMSTUDIO_MODEL` (default `lmstudio:local`).

---

## Authentication

The TUI **AuthPicker** (first-run wizard or `elai login` with no flags) lists every supported method. Cloud providers persist creds in `~/.config/elai/credentials.json`; local providers (Ollama / LM Studio) need only the `*_BASE_URL` env var.

| Method (TUI label)            | Stored as `AuthMethod`            | When to use                                                              |
| ----------------------------- | --------------------------------- | ------------------------------------------------------------------------ |
| Claude.ai OAuth (Pro/Max)     | `ClaudeAiOAuth`                   | Anthropic subscribers — OAuth via claude.ai                              |
| Console OAuth                 | `ConsoleApiKey`                   | Anthropic Console — OAuth that creates an API key for you                |
| SSO OAuth                     | `ClaudeAiOAuth` w/ SSO            | Enterprise SSO — claude.ai flow with `login_method=sso`                  |
| Colar API key (`sk-ant-...`)  | `ConsoleApiKey { origin: Pasted }` | Paste an Anthropic API key                                               |
| Colar Auth Token (Bearer)     | `AnthropicAuthToken`              | Paste an `ANTHROPIC_AUTH_TOKEN` bearer                                   |
| Colar OpenAI key (`sk-...`)   | `OpenAiApiKey`                    | Paste an OpenAI key — used as fallback when `OPENAI_API_KEY` is unset    |
| AWS Bedrock                   | `Bedrock`                         | Sets `CLAUDE_CODE_USE_BEDROCK=1`; AWS creds via standard chain           |
| Google Vertex AI              | `Vertex`                          | Sets `CLAUDE_CODE_USE_VERTEX=1`                                          |
| Azure Foundry                 | `Foundry`                         | Sets `CLAUDE_CODE_USE_FOUNDRY=1`                                         |
| Importar Claude Code creds    | `ClaudeAiOAuth`                   | Import from `~/.claude/credentials.json` (no interaction)                |
| Elai OAuth legacy             | `ClaudeAiOAuth` (legacy issuer)   | Legacy `elai.dev` OAuth — kept for upgrade paths                         |

**Local providers** (Ollama, LM Studio) don't appear in the AuthPicker — they're auto-detected when `OLLAMA_BASE_URL` or `LMSTUDIO_BASE_URL` is set, and accept requests without a real key (Elai sends the placeholder `"ollama"` / `"lm-studio"` as a no-op `Authorization` header).

**CLI shortcuts:** `elai login` also accepts flags as shortcuts for the most common methods:

| Flag                    | TUI equivalent                              |
| ----------------------- | ------------------------------------------- |
| `--claudeai`            | Claude.ai OAuth                             |
| `--console`             | Console OAuth                               |
| `--sso`                 | SSO OAuth                                   |
| `--api-key`             | Colar API key (interactive prompt or `--stdin`) |
| `--token`               | Colar Auth Token                            |
| `--use-bedrock`         | AWS Bedrock                                 |
| `--use-vertex`          | Google Vertex AI                            |
| `--use-foundry`         | Azure Foundry                               |
| `--import-claude-code`  | Importar Claude Code credentials            |
| `--legacy-elai`         | Elai OAuth legacy                           |

Useful flags for any method:

- `--email <addr>` pre-fills the OAuth login page (`login_hint`).
- `--no-browser` prints the OAuth URL instead of opening one (CI / remote shells).
- `--stdin` reads the secret from stdin (only with `--api-key` / `--token`).

Inspect / list methods:

```bash
elai auth status        # active method, expiry, scopes
elai auth list          # all available methods
elai logout             # clear saved credentials
```

---

## Indexing & Embeddings

`elai init` creates `.elai/`, writes a starter `ELAI.md`, and indexes the codebase for semantic search.

```bash
elai init                          # default: SQLite + local fastembed
elai init --backend qdrant --qdrant-url http://localhost:6333
elai init --embed-provider ollama --ollama-url http://localhost:11434
elai init --no-index               # skip indexing, only scaffold .elai/ + ELAI.md
elai init --reindex                # wipe existing index and rebuild
```

| Flag                   | Values                                            | Default       |
| ---------------------- | ------------------------------------------------- | ------------- |
| `--backend`            | `sqlite` (vec-sqlite), `qdrant`                   | `sqlite`      |
| `--embed-provider`     | `local` (fastembed), `ollama`, `jina`, `openai`, `voyage` | `local` |
| `--embed-model`        | model name (overrides provider default)           | provider auto |
| `--qdrant-url`         | URL of running Qdrant instance                    | —             |
| `--ollama-url`         | URL of running Ollama instance                    | —             |
| `--no-watcher`         | don't spawn the background re-indexer             | false         |
| `--no-index`           | scaffold only, skip indexing                      | false         |
| `--reindex`            | drop existing index and rebuild from scratch      | false         |

Notes:

- The **`local`** embed provider requires a build with `embed-fastembed` (default cargo feature). It is **not** available in musl Linux binaries (x86_64 / arm64) and macOS x86_64 binaries — those builds ship without `fastembed`. Use `--embed-provider ollama` or any HTTP provider on those targets.
- After `init`, a background watcher keeps the index in sync. Manage it with `/cache stats` and `/cache clear` from inside the REPL.

---

## Plugins, Skills & Agents

| Concept | Lives in | What it is |
| ------- | -------- | ---------- |
| **Plugin** | `~/.elai/plugins/<id>/` (configurable via `install_root`) | Versioned bundle that adds tools, skills, or hooks. Has `metadata.toml` (id, name, version) and a manifest. |
| **Skill** | `.elai/skills/<name>/SKILL.md`, `.codex/skills/`, legacy `/commands/` | Markdown file with YAML frontmatter (`name`, `description`, `priority`, `budget_multiplier`, `force_provider`, `incompatible_with`). Auto-loaded into the prompt when keywords match. |
| **Agent** | `.elai/agents/`, `.codex/agents/`, `$CODEX_HOME/agents` | Sub-agent definition (markdown + frontmatter). Triggered by tool dispatch or the orchestration pipeline. |

Plugin commands:

```text
/plugin                          # list installed plugins
/plugin install <path-or-url>    # install from a local path or git URL
/plugin enable <name>            # enable a previously installed plugin
/plugin disable <name>           # disable without uninstalling
/plugin uninstall <id>           # remove from disk
/plugin update <id>              # pull and re-install latest
```

Skills and agents are listed (and explained) by:

```text
/skills                          # discovered skills + origin/priority
/agents                          # discovered agent definitions
```

Skills and agents are file-driven — drop a `SKILL.md` or agent file in the discovery path and it shows up on the next reload. No install step.

---

## Slash Commands

35 commands grouped by purpose. Run `/help` inside the REPL for the live, runtime-filtered list.

### Session

| Command | Description |
| --- | --- |
| `/help` | Show available slash commands |
| `/status` | Show current session status |
| `/compact` | Compact local session history |
| `/clear [--confirm]` | Start a fresh local session |
| `/cost` | Show cumulative token usage for this session |
| `/resume <session-path>` | Load a saved session into the REPL |
| `/export [file]` | Export the current conversation to a file |

### Behavior

| Command | Description |
| --- | --- |
| `/model [name]` | Show or switch the active model |
| `/permissions [read-only\|workspace-write\|danger-full-access]` | Show or switch the active permission mode |
| `/tools [why]` | Inspect tool selection (`why` explains the last decision) |
| `/budget [tokens] [usd] \| off` | Set or clear the per-session budget cap |
| `/cache [clear\|stats]` | Manage the response/index cache |
| `/providers [--verbose]` | Show provider usage dashboard |

### Project

| Command | Description |
| --- | --- |
| `/init` | Create a starter `ELAI.md` for this repo |
| `/memory` | Inspect loaded Elai instruction memory files |
| `/config [env\|hooks\|model\|plugins]` | Inspect Elai config files or merged sections |
| `/verify` | Verify codebase files against memory entries |

### Git

| Command | Description |
| --- | --- |
| `/diff` | Show git diff for current workspace changes |
| `/branch [list\|create <name>\|switch <name>]` | List, create, or switch git branches |
| `/worktree [list\|add <path> [branch]\|remove <path>\|prune]` | Manage git worktrees |
| `/commit` | Generate a commit message and create a git commit |
| `/commit-push-pr [context]` | Commit, push, and open a PR in one step |
| `/pr [context]` | Draft or create a pull request from the conversation |
| `/issue [context]` | Draft or create a GitHub issue from the conversation |

### Analysis

| Command | Description |
| --- | --- |
| `/bughunter [scope]` | Inspect the codebase for likely bugs |
| `/ultraplan [task]` | Run a deep planning prompt with multi-step reasoning |
| `/teleport <symbol-or-path>` | Jump to a file or symbol by searching the workspace |
| `/debug-tool-call` | Replay the last tool call with debug details |

### Plugins / Skills / Agents

| Command | Description |
| --- | --- |
| `/plugin [list\|install\|enable\|disable\|uninstall\|update]` | Manage Elai Code plugins |
| `/skills` | List available skills |
| `/agents` | List configured agents |
| `/session [list\|switch <session-id>]` | List or switch managed local sessions |
| `/dream [--force]` | Compress old memory entries into a summary |
| `/stats [--days N] [--by-model] [--by-project]` | Token usage and cost statistics |

### System

| Command | Description |
| --- | --- |
| `/version` | Show CLI version and build information |
| `/update` | Check for and install the latest Elai Code release |
| `/swd [off\|partial\|full]` | Toggle or set Strict Write Discipline level |

---

## Architecture

```
rust/
├── crates/
│   ├── api/            # HTTP client — streaming SSE, provider abstraction, OAuth
│   ├── runtime/        # Conversation loop, config, session persistence, MCP orchestration
│   ├── tools/          # Tool registry, TOML catalog, execution framework, MCP tools
│   ├── elai-cli/       # Interactive REPL, TUI (ratatui), SWD engine, CLI parsing
│   ├── commands/       # Slash command registry (/help /model /cost /swd /diff …)
│   ├── plugins/        # Plugin lifecycle and hook pipeline
│   ├── server/         # HTTP/SSE server (axum) for headless use
│   ├── lsp/            # LSP client integration
│   └── compat-harness/ # Compatibility layer for editor integrations
src/                    # Python reference workspace (audit and parity surface)
tests/                  # Python verification suite
```

---

## Configuration

Elai Code reads a layered config from `~/.elai/config.toml` (user) and `.elai/config.toml` (project). Environment variables override file values.

```toml
[model]
default = "claude-sonnet-4-6"

[budget]
max_usd = 5.00

[swd]
level = "partial"

[permissions]
mode = "workspace-write"
```

---

## Releasing

Maintainers publish a new version with a single command:

```bash
./scripts/release.sh 0.7.2     # next patch — pass the new version number
```

Pré-requisitos:

- estar na branch `main`,
- working tree limpo (sem mudanças não commitadas),
- a tag `v<versão>` ainda não existir no repositório.

O script automatiza tudo o que precisa entrar na release:

1. bumpa `version` em `rust/Cargo.toml`,
2. recompila para atualizar `rust/Cargo.lock`,
3. gera changelog a partir dos commits desde a última tag (filtra `chore: bump version`, `Merge`, `docs: atualiza README`),
4. atualiza a seção "What's New" do README,
5. cria commit `chore: bump version to v<versão>`,
6. cria a tag anotada `v<versão>`,
7. faz `git push --follow-tags`.

O push da tag dispara o workflow [`Release`](.github/workflows/release.yml), que builda os binários para os 5 alvos (macOS arm64/x86_64, Linux musl x86_64/arm64, Windows x86_64) e publica em [Releases](https://github.com/nextlw/elai-code/releases) com `checksums.txt`.

Acompanhe o build:

```bash
gh run list --workflow=release.yml --limit 1
gh run watch <run-id> --exit-status
```

Se algum job falhar, **não** retague a versão — corrija o problema, commite o fix em `main`, e publique uma nova patch (`0.7.2 → 0.7.3`). Retag em massa quebra clones já existentes.

### Cloudflare Worker (instalador)

O endpoint `https://get.nexcode.live` é provisionado uma única vez por:

```bash
./scripts/setup-cf-installer.sh
```

Ele lê credenciais do arquivo `.env` na raiz do repo (chaves `CLOUD_FLARE_EMAIL`, `CLOUD_FLARE_API_KEY`, `CLOUD_FLARE_ACCOUNT_ID`, `CLOUD_FLARE_ZONE_ID`, `CLOUD_FLARE_ZONE_DOMAIN`), faz upload do Worker `elai-installer` e amarra o domínio custom. Como o Worker apenas proxia para `scripts/install.sh` / `scripts/install.ps1` no `main` do GitHub, ele não precisa ser re-deployado a cada release — só quando os scripts de instalação mudarem.

---

## Contributing

Pull requests are welcome. Open an issue first for large changes.

1. Fork and create a feature branch
2. `cargo fmt && cargo clippy` before committing
3. Add tests where relevant

---

## License

MIT © [Nexcode](https://nexcode.live)
