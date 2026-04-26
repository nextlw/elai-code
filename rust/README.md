# 🚀 Elai GenAI — Rust Implementation

A high-performance Rust rewrite of the Elai GenAI CLI agent harness. Built for speed, safety, and native tool execution.

> Formerly known as *Elai Code*. Rebranded to **Elai GenAI** with new features including Strict Write Discipline (SWD), visual identity refresh, and TUI enhancements.

## Quick Start

```bash
# Build
cd rust/
cargo build --release

# Run interactive TUI REPL
./target/release/elai

# One-shot prompt
./target/release/elai prompt "explain this codebase"

# With specific model
./target/release/elai --model sonnet prompt "fix the bug in main.rs"

# With SWD full mode (transactional writes)
./target/release/elai --swd full
```

## Configuration

Set your API credentials:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# Or use a proxy
export ANTHROPIC_BASE_URL="https://your-proxy.com"
```

Or authenticate via OAuth:

```bash
elai login
```

## Features

| Feature | Status |
|---------|--------|
| API + streaming | ✅ |
| OAuth login/logout | ✅ |
| Interactive TUI REPL (ratatui + crossterm) | ✅ |
| Tool system (bash, read, write, edit, grep, glob) | ✅ |
| Web tools (search, fetch) | ✅ |
| Sub-agent orchestration | ✅ |
| Todo tracking | ✅ |
| Notebook editing | ✅ |
| ELAI.md / project memory | ✅ |
| Config file hierarchy (.elai.json) | ✅ |
| Permission system | ✅ |
| MCP server lifecycle | ✅ |
| Session persistence + resume | ✅ |
| Extended thinking (thinking blocks) | ✅ |
| Cost tracking + usage display | ✅ |
| Git integration | ✅ |
| Markdown terminal rendering (ANSI) | ✅ |
| Model aliases (opus/sonnet/haiku) | ✅ |
| Slash commands (/status, /compact, /clear, etc.) | ✅ |
| **Strict Write Discipline (SWD)** | ✅ 🆕 |
| **GPT-4o-mini pricing** | ✅ 🆕 |
| **Animated spinner & word-wrap** | ✅ 🆕 |
| Hooks (PreToolUse/PostToolUse) | 🔧 Config only |
| Plugin system | 📋 Planned |
| Skills registry | 📋 Planned |

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
# CLI flag
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

The engine:
1. Snapshots all target files before execution
2. Executes all actions sequentially
3. Verifies content hashes match
4. Rolls back **all** actions if any single one fails

### Audit Log

All SWD transactions are appended to `.elai/swd.log` as JSON-lines:

```json
{"ts":1711843200000,"tool":"write_file","path":"src/main.rs","outcome":"Verified","before":"abc123...","after":"def456..."}
```

### TUI Integration

- SWD level is displayed in the status bar footer: `SWD:partial`
- Dedicated `SwdLogEntry` chat widget shows per-file results with icons:
  - ✓ (green) — Verified
  - · (yellow) — Noop
  - ~ (yellow) — Drift (hash mismatch)
  - ✗ (red) — Failed
  - ↩ (red) — Rolled back

## Model Aliases

Short names resolve to the latest model versions:

| Alias | Resolves To |
|-------|------------|
| `opus` | `claude-opus-4-6` |
| `sonnet` | `claude-sonnet-4-6` |
| `haiku` | `claude-haiku-4-5-20251213` |

## Supported Model Pricing

| Model | Input $/M | Output $/M |
|-------|-----------|------------|
| Claude Opus | 15.00 | 75.00 |
| Claude Sonnet | 3.00 | 15.00 |
| Claude Haiku | 0.25 | 1.25 |
| **GPT-4o-mini** 🆕 | 0.15 | 0.60 |

## CLI Flags

```
elai [OPTIONS] [COMMAND]

Options:
  --model MODEL                    Set the model (alias or full name)
  --dangerously-skip-permissions   Skip all permission checks
  --permission-mode MODE           Set read-only, workspace-write, or danger-full-access
  --allowedTools TOOLS             Restrict enabled tools
  --swd LEVEL                      Strict Write Discipline: off, partial (default), full
  --no-tui                         Disable TUI, use plain text REPL
  --output-format FORMAT           Output format (text or json)
  --version, -V                    Print version info

Commands:
  prompt <text>      One-shot prompt (non-interactive)
  login              Authenticate via OAuth
  logout             Clear stored credentials
  init               Initialize project config
  doctor             Check environment health
  self-update        Update to latest version
```

## Slash Commands (REPL)

| Command | Description |
|---------|-------------|
| `/help` | Show help |
| `/status` | Show session status (model, tokens, cost) |
| `/cost` | Show cost breakdown |
| `/compact` | Compact conversation history |
| `/clear` | Clear conversation |
| `/model [name]` | Show or switch model |
| `/permissions` | Show or switch permission mode |
| `/config [section]` | Show config (env, hooks, model) |
| `/memory` | Show ELAI.md contents |
| `/diff` | Show git diff |
| `/export [path]` | Export conversation |
| `/session [id]` | Resume a previous session |
| `/swd [off\|partial\|full]` | 🆕 Show or change SWD level |
| `/version` | Show version |

**Keyboard shortcuts:** F2=model · F3=permissions · F4=sessions · Ctrl+K=palette

## Workspace Layout

```
rust/
├── Cargo.toml              # Workspace root
├── Cargo.lock
└── crates/
    ├── api/                # API client + SSE streaming
    ├── commands/           # Shared slash-command registry
    ├── compat-harness/     # TS manifest extraction harness
    ├── runtime/            # Session, config, permissions, MCP, prompts, pricing
    ├── elai-cli/           # Main CLI binary
    │   └── src/
    │       ├── main.rs     # CLI entry, REPL, runtime wiring
    │       ├── tui.rs      # TUI (ratatui) — chat, overlays, SWD widget
    │       ├── swd.rs      # 🆕 Strict Write Discipline engine
    │       ├── render.rs   # Markdown → ANSI renderer
    │       └── init.rs     # Project bootstrap
    └── tools/              # Built-in tool implementations
```

### Crate Responsibilities

- **api** — HTTP client, SSE stream parser, request/response types, auth (API key + OAuth bearer)
- **commands** — Slash command definitions and help text generation
- **compat-harness** — Extracts tool/prompt manifests from upstream TS source
- **runtime** — `ConversationRuntime` agentic loop, `ConfigLoader` hierarchy, `Session` persistence, permission policy, MCP client, system prompt assembly, usage tracking, model pricing
- **elai-cli** — TUI REPL, one-shot prompt, streaming display, tool call rendering, CLI argument parsing, **SWD engine**
- **tools** — Tool specs + execution: Bash, ReadFile, WriteFile, EditFile, GlobSearch, GrepSearch, WebSearch, WebFetch, Agent, TodoWrite, NotebookEdit, Skill, ToolSearch, REPL runtimes

## Stats

- **~20K+ lines** of Rust
- **6 crates** in workspace
- **Binary name:** `elai`
- **Default model:** `claude-opus-4-6`
- **Default permissions:** `danger-full-access`
- **Default SWD:** `partial`

## License

See repository root.
