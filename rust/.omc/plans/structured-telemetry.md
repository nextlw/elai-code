# Plan: Structured Telemetry for Claw CLI

**Date:** 2026-04-26
**Complexity:** MEDIUM
**Scope:** 5 tasks across ~8 files (1 new module, 6 modified, 1 new test file)

---

## Context

The TypeScript reference (`mythos-router`) uses SQLite with a batching queue (2s / 10 events) to persist routing decisions, provider metrics, and failures. For Claw (Rust), we adopt JSON-lines (`~/.config/claw/telemetry.jsonl`) as the storage backend to avoid the `rusqlite` dependency. The existing `UsageTracker` in `crates/runtime/src/usage.rs` tracks per-session token counts but has no persistence -- telemetry fills that gap.

**Key design decisions carried from the TS reference:**
- Async batch queue (mpsc channel) so the main thread never blocks on I/O
- Flush on threshold (10 events) OR timer (5 seconds), whichever comes first
- Retention cap (10,000 lines) with automatic rotation
- Graceful shutdown flushes pending events before process exit

---

## Guardrails

### Must Have
- Zero impact on request latency (all I/O on background task)
- Serde-based serialization with `#[serde(tag = "type")]` for easy grep/jq
- Telemetry disabled when `CLAW_TELEMETRY=off` env var is set
- All events carry a `timestamp_ms: u64` (Unix epoch millis)

### Must NOT Have
- No SQLite dependency
- No network calls (local-only persistence)
- No blocking file I/O on the hot path (API call thread)
- No changes to the `ApiClient` trait signature

---

## Task Flow

### Task 1 -- New module `crates/runtime/src/telemetry.rs`

**What:** Create the telemetry data model, async store, and flush logic.

**Structs and enums to create:**

```rust
// -- Event types (tagged enum, serde-serializable) --

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TelemetryEvent {
    ProviderSelected {
        timestamp_ms: u64,
        provider: String,
        model: String,
        latency_ms: u64,
        reason: String,        // e.g. "default", "fallback", "user-override"
    },
    RequestFailed {
        timestamp_ms: u64,
        provider: String,
        error: String,
        retried: bool,
    },
    TokenUsage {
        timestamp_ms: u64,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
        cost_usd: f64,
    },
    SessionEnd {
        timestamp_ms: u64,
        session_id: String,
        turns: u32,
        total_cost_usd: f64,
        duration_ms: u64,
    },
}

// -- Store handle (cheap Clone, send to any thread) --

#[derive(Clone)]
pub struct TelemetryHandle {
    tx: mpsc::UnboundedSender<TelemetryEvent>,
}

// -- Background worker (owns the receiver + file) --

pub struct TelemetryWorker { /* private */ }
```

**Behavior:**
- `TelemetryHandle::emit(event)` sends via unbounded mpsc (never blocks).
- `TelemetryWorker` runs as a `tokio::spawn` task. It collects events into a `Vec<TelemetryEvent>` buffer.
- Flush trigger: `tokio::select!` over (a) buffer reaching 10 events, (b) `tokio::time::interval(Duration::from_secs(5))` tick, (c) a oneshot shutdown signal.
- Flush writes each event as one JSON line (`serde_json::to_string` + `\n`) via `tokio::fs::OpenOptions` append mode.
- After flush, check line count. If > 10,000, truncate by reading all lines, keeping the last 8,000, rewriting the file (simple rotation -- no archive).
- `TelemetryHandle::noop()` returns a handle whose channel is immediately closed (for `CLAW_TELEMETRY=off` or test contexts).

**File path:** `crates/runtime/src/telemetry.rs`

**Dependencies to add to `crates/runtime/Cargo.toml`:**
- `dirs = "6"` (for `dirs::config_dir()` to get `~/.config/claw/`)
- `chrono` is NOT needed -- use `std::time::SystemTime` for timestamps

**Wire into `crates/runtime/src/lib.rs`:**
- Add `mod telemetry;`
- Re-export: `pub use telemetry::{TelemetryEvent, TelemetryHandle, TelemetryWorker};`

**Acceptance criteria:**
- [ ] `TelemetryEvent` round-trips through `serde_json` (serialize then deserialize) for all 4 variants
- [ ] `TelemetryHandle::emit()` returns immediately (no `.await`)
- [ ] Worker flushes to file within 5s even if fewer than 10 events are buffered
- [ ] File stays under 10,000 lines after sustained writes
- [ ] `TelemetryHandle::noop()` silently drops events with no panic or error

---

### Task 2 -- Integration into `ConversationRuntime`

**What:** Add an optional `TelemetryHandle` to `ConversationRuntime` so every turn emits `TokenUsage` events automatically.

**Changes:**

1. **`crates/runtime/src/conversation.rs`** -- Add field to `ConversationRuntime<C, T>`:
   ```rust
   telemetry: TelemetryHandle,  // defaults to noop
   ```
   - In `new()` and `new_with_features()`: default to `TelemetryHandle::noop()`.
   - Add builder method: `pub fn with_telemetry(mut self, handle: TelemetryHandle) -> Self`.
   - In `run_turn()`, after `self.usage_tracker.record(usage)` (line ~181 of conversation.rs), emit:
     ```rust
     self.telemetry.emit(TelemetryEvent::TokenUsage {
         timestamp_ms: now_millis(),
         model: /* need model name -- see note below */,
         input_tokens: usage.input_tokens,
         output_tokens: usage.output_tokens,
         cache_read_tokens: usage.cache_read_input_tokens,
         cache_write_tokens: usage.cache_creation_input_tokens,
         cost_usd: usage.estimate_cost_usd().total_cost_usd(),
     });
     ```

2. **Model name propagation:** `ConversationRuntime` currently does not know the model name. Add an optional `model_name: Option<String>` field (set via builder `.with_model_name()`). If `None`, the `TokenUsage` event uses `"unknown"`. This avoids changing the `ApiClient` trait.

**Acceptance criteria:**
- [ ] Existing tests in `conversation.rs` still pass (noop handle adds no side effects)
- [ ] When a `TelemetryHandle` is wired in, each `run_turn` emits exactly one `TokenUsage` event per API response that includes usage data
- [ ] Model name appears in the event when provided

---

### Task 3 -- Wire telemetry in `claw-cli/src/main.rs`

**What:** Start the `TelemetryWorker`, pass the handle through to the runtime, emit `ProviderSelected` and `SessionEnd` events, and flush on exit.

**Changes to `crates/claw-cli/src/main.rs`:**

1. **Startup (`build_runtime` / `LiveCli::new`):**
   - Check `std::env::var("CLAW_TELEMETRY")`. If `"off"`, use `TelemetryHandle::noop()`.
   - Otherwise, call `TelemetryWorker::start()` which returns `(TelemetryHandle, TelemetryShutdown)`. The `TelemetryShutdown` is a oneshot sender that triggers final flush.
   - Pass `TelemetryHandle` into the runtime via `.with_telemetry(handle)`.
   - Store `TelemetryShutdown` in `LiveCli` (or at the `run_repl` / `run_tui_repl` scope level).

2. **`DefaultRuntimeClient::stream()`** -- After the streaming call resolves, emit `ProviderSelected`:
   ```rust
   telemetry.emit(TelemetryEvent::ProviderSelected {
       timestamp_ms: now_millis(),
       provider: self.client.provider_name().to_string(),
       model: self.model.clone(),
       latency_ms: start.elapsed().as_millis() as u64,
       reason: "default".into(),
   });
   ```
   On error, emit `RequestFailed`.

3. **Graceful shutdown:**
   - In `run_repl`, before `Ok(())`: drop or signal `TelemetryShutdown`. The worker's `tokio::select!` detects the closed channel and performs a final flush.
   - In `run_tui_repl`, same pattern in the cleanup block after the TUI loop exits.

4. **`SessionEnd` event:** Emit in `LiveCli::persist_session()` or at REPL exit, pulling `turns` and `total_cost` from the `UsageTracker`, and `duration_ms` from a session start `Instant`.

**Acceptance criteria:**
- [ ] `claw` starts without error when `CLAW_TELEMETRY` is unset (telemetry active by default)
- [ ] `CLAW_TELEMETRY=off claw -p "hi"` produces no telemetry file
- [ ] After a normal session, `~/.config/claw/telemetry.jsonl` contains at least `ProviderSelected` and `TokenUsage` events
- [ ] On `/exit` or Ctrl-C, pending events are flushed before process terminates
- [ ] `DefaultRuntimeClient` field additions do not change its public API (it is a private struct)

---

### Task 4 -- `claw stats` command

**What:** Add a `stats` subcommand that reads `telemetry.jsonl` and prints an ASCII summary table.

**Changes:**

1. **`CliAction` enum:** Add variant `Stats`.
2. **`parse_args`:** Match `"stats"` as a positional subcommand.
3. **New function `run_stats()`:**
   - Read `~/.config/claw/telemetry.jsonl` line by line.
   - Deserialize each line as `TelemetryEvent`.
   - Aggregate:
     - **By model:** total requests, total input/output tokens, total cost, avg latency
     - **By date (YYYY-MM-DD):** daily cost, daily token count
     - **Failures:** count by provider, last error message
   - Print as a formatted ASCII table (use simple `format!` padding, no extra crate needed).

   Example output:
   ```
   Model                Requests   Tokens (in/out)    Cost       Avg Latency
   ────────────────────────────────────────────────────────────────────────
   claude-sonnet-4-6       42     1.2M / 340K       $12.4500       1.8s
   gpt-4.1-mini            15     200K / 80K         $0.2100       0.9s

   Daily Summary (last 7 days)
   ────────────────────────────────────────────────────────────────────────
   2026-04-26              $3.2100     420K tokens
   2026-04-25              $5.1200     890K tokens
   ```

4. **Wire in `run()`:** Add `CliAction::Stats => run_stats(),` to the match block.

**Acceptance criteria:**
- [ ] `claw stats` runs without error even when no telemetry file exists (prints "No telemetry data found")
- [ ] Handles malformed lines gracefully (skips them, does not panic)
- [ ] Output is human-readable with aligned columns
- [ ] Aggregation correctly sums tokens and costs across events

---

### Task 5 -- Tests

**What:** Unit tests for serialization, flush logic, and stats aggregation.

**Files:**
- Tests inside `crates/runtime/src/telemetry.rs` (inline `#[cfg(test)] mod tests`)
- Integration-level test in `crates/claw-cli/tests/` if the project has integration tests (otherwise inline)

**Test cases:**

1. **Serde round-trip:** Each `TelemetryEvent` variant serializes to JSON, the JSON contains `"type": "VariantName"`, and deserializes back to an equal value.

2. **Flush-on-threshold:** Create a worker with a temp file, emit 10 events, assert the file has 10 lines within 1 second.

3. **Flush-on-timer:** Emit 3 events, wait 6 seconds, assert file has 3 lines.

4. **Retention rotation:** Write 12,000 events, trigger flush, assert file has <= 10,000 lines (specifically ~8,000 after truncation).

5. **Noop handle:** `TelemetryHandle::noop().emit(event)` does not panic.

6. **Graceful shutdown:** Drop `TelemetryShutdown`, assert all buffered events are flushed before the worker task completes.

7. **Stats aggregation (unit):** Given a known set of JSON lines, verify the aggregation logic produces correct totals per model and per date.

**Acceptance criteria:**
- [ ] `cargo test -p runtime` passes with all new telemetry tests green
- [ ] No flaky timing issues (use `tokio::time::pause()` for timer-dependent tests)
- [ ] Tests clean up temp files

---

## Dependency Summary

| Crate | Addition | Why |
|-------|----------|-----|
| `crates/runtime/Cargo.toml` | `dirs = "6"` | Resolve `~/.config/claw/` cross-platform |
| (already present) | `tokio` with `fs`, `sync`, `time` | Async worker, mpsc, interval timer |
| (already present) | `serde`, `serde_json` | Event serialization |

**Note:** Check if tokio features need `"fs"` and `"sync"` added -- currently the runtime Cargo.toml has `["io-util", "macros", "process", "rt", "rt-multi-thread", "time"]`. Add `"fs"` and `"sync"` to the feature list.

---

## File Change Map

| File | Action | Description |
|------|--------|-------------|
| `crates/runtime/Cargo.toml` | MODIFY | Add `dirs = "6"`, add tokio features `"fs"`, `"sync"` |
| `crates/runtime/src/telemetry.rs` | CREATE | `TelemetryEvent`, `TelemetryHandle`, `TelemetryWorker`, flush logic, retention, tests |
| `crates/runtime/src/lib.rs` | MODIFY | Add `mod telemetry;` and re-exports |
| `crates/runtime/src/conversation.rs` | MODIFY | Add `telemetry` + `model_name` fields, emit `TokenUsage` in `run_turn` |
| `crates/claw-cli/src/main.rs` | MODIFY | Wire telemetry startup/shutdown, emit `ProviderSelected`/`RequestFailed`/`SessionEnd`, add `Stats` action, `run_stats()` |
| `crates/claw-cli/Cargo.toml` | MODIFY | (only if needed for `dirs` re-export -- likely not, since runtime already re-exports) |

---

## Success Criteria

1. `cargo build` compiles with no warnings
2. `cargo test -p runtime` passes all existing + new tests
3. Running `claw` interactively produces `~/.config/claw/telemetry.jsonl` with correctly structured events
4. `claw stats` prints a readable summary table
5. `CLAW_TELEMETRY=off claw -p "test"` produces no telemetry file
6. Ctrl-C during a session still flushes pending events
7. Sustained usage does not grow the file beyond ~10,000 lines
