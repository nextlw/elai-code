# Elai Code

<p align="center"><pre>
  ██████████████████   ███████╗██╗      █████╗ ██╗
  ████████░░▄▄░░▄▄░░   ██╔════╝██║     ██╔══██╗██║
  ████████░░██░░██░░   █████╗  ██║     ███████║██║
  ████████░░▀▀░░▀▀░░   ██╔══╝  ██║     ██╔══██║██║
  ██████████████████   ███████╗███████╗██║  ██║██║
        ████  ████     ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝
</pre></p>

<p align="center">
  <strong>A high-performance CLI agent harness built in Rust by <a href="https://nexcode.live">Nexcode</a></strong>
</p>

<p align="center">
  <a href="https://github.com/nextlw/elai-code/releases"><img src="https://img.shields.io/github/v/release/nextlw/elai-code?style=for-the-badge&color=orange" alt="Release" /></a>
  <a href="https://github.com/nextlw/elai-code/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=for-the-badge" alt="License" /></a>
  <img src="https://img.shields.io/badge/built_with-Rust-orange?style=for-the-badge&logo=rust" alt="Rust" />
</p>

---

Elai Code is a modular, memory-safe agent harness that lets AI models safely interact with your filesystem, codebase, web resources, and remote agents — with transactional write guarantees, real-time cost tracking, and a polished terminal UI.

## Features

### Strict Write Discipline (SWD)

A transactional filesystem write engine with three operating levels:

| Level | Behavior |
|-------|----------|
| `off` | Normal tool execution — no interception |
| `partial` *(default)* | Wraps every write with SHA-256 snapshots, automatic rollback on failure, and a JSON-lines audit log |
| `full` | Blocks all write tools; the model emits structured `[FILE_ACTION]` blocks executed transactionally with hash verification and full rollback |

```
--swd off|partial|full      CLI flag
/swd [off|partial|full]     REPL command (cycles levels when called without argument)
```

### Multi-Provider Support

| Provider | Models |
|----------|--------|
| Anthropic | `claude-opus-4-6`, `claude-sonnet-4-6`, `claude-haiku-4-5` (and aliases `opus`, `sonnet`, `haiku`) |
| OpenAI-compatible | `gpt-4o-mini`, any OpenAI-compatible proxy |

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

### Requirements

- Rust 1.77+ with Cargo
- An Anthropic API key (or OpenAI-compatible endpoint)

### Build

```bash
git clone https://github.com/nextlw/elai-code.git
cd elai-code/rust
cargo build --release
```

The binary is at `rust/target/release/elai`.

### Run

```bash
export ANTHROPIC_API_KEY=your_key_here
./elai
```

Or point to an OpenAI-compatible endpoint:

```bash
export OPENAI_API_KEY=your_key_here
./elai --model gpt-4o-mini --api-base https://your-proxy/v1
```

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

### Key Slash Commands

| Command | Description |
|---------|-------------|
| `/model [name]` | Switch model mid-session |
| `/cost` | Show session token usage and USD cost |
| `/swd [level]` | Toggle or set Strict Write Discipline level |
| `/diff` | Show git diff of workspace changes |
| `/compact` | Compress conversation context |
| `/tools` | List available tools |
| `/tools why` | Explain current tool rate-limit decisions |
| `/status` | Show session status and config |

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

## Python Workspace

The `src/` tree is a Python reference surface used for porting audits and integration tests. It is **not** the production runtime — the Rust CLI is the canonical implementation.

```bash
# Render porting summary
python3 -m src.main summary

# List current Python modules
python3 -m src.main subsystems --limit 16

# Run verification
python3 -m unittest discover -s tests -v
```

---

## Contributing

Pull requests are welcome. Open an issue first for large changes.

1. Fork and create a feature branch
2. `cargo fmt && cargo clippy` before committing
3. Add tests where relevant

---

## License

MIT © [Nexcode](https://nexcode.live)
