# Plan: Deterministic Response Cache for Elai CLI

**Date:** 2026-04-26
**Complexity:** MEDIUM
**Scope:** 5 files modified, 2 files created, 1 dependency added

---

## Context

The TypeScript reference implementation (`mythos-router/src/cache.ts`) uses:
- **Canonical JSON** (recursive sorted keys) + SHA-256 for deterministic cache keys
- **SQLite backend** via `node:sqlite`
- **TTL 1h** default, eviction on read
- **Tool call bypass** (invariant: responses with `toolCalls.length > 0` are never cached)
- **Lazy init** with graceful fallback (cache disabled if SQLite unavailable)

The Rust project already has `sha2 = "0.10"` in `crates/runtime/Cargo.toml` and a hand-rolled `json.rs` module using `BTreeMap` (keys are already sorted). The `ConversationClient::run_turn` in the CLI layer is the natural interception point.

---

## Decision: JSON File vs rusqlite

**Recommendation: JSON file first, upgrade path to rusqlite later.**

| Factor | JSON File | rusqlite |
|---|---|---|
| New dependencies | 0 | `rusqlite` (~1.5MB compile, C linkage) |
| Concurrent access | File lock (flock) | Built-in WAL mode |
| Query performance | O(n) scan on load | O(1) key lookup |
| TTL eviction | Full rewrite on evict | Single DELETE statement |
| Complexity | ~120 LOC | ~180 LOC |

For a CLI tool with low concurrency (single process), JSON file is sufficient for v1. The `ResponseCache` trait boundary ensures a future SQLite swap is a single-file change. If the cache grows beyond ~1000 entries, revisit.

---

## Work Objectives

1. Create `response_cache.rs` module in `crates/runtime/src/`
2. Wire cache into the request pipeline (before provider call, after response)
3. Add `/cache clear` slash command and `--no-cache` CLI flag
4. Enforce the tool-call bypass invariant at both input and output boundaries
5. Add comprehensive tests

---

## Guardrails

### Must Have
- Requests containing `ToolUse` or `ToolResult` content blocks in ANY message NEVER produce cache hits or writes
- Canonical JSON serialization is deterministic (sorted keys -- already guaranteed by `BTreeMap` in `json.rs`)
- Cache is non-critical: all errors are silently swallowed, never crash the CLI
- TTL default 1 hour, configurable
- `--no-cache` disables cache entirely for a session

### Must NOT Have
- No `rusqlite` dependency in v1 (keep zero native C deps)
- No caching of streaming responses (only the final assembled `TurnSummary` / `MessageResponse`)
- No cache warming or prefetching
- No shared cache between concurrent CLI instances (file lock is sufficient, not required)

---

## Task Flow

### Step 1: `crates/runtime/src/response_cache.rs` -- Core Module

**New file:** `crates/runtime/src/response_cache.rs`

```rust
// Core types and implementation
pub struct CacheKey(String); // SHA-256 hex digest

pub struct CachedResponse {
    pub response_json: String,      // serialized MessageResponse or TurnSummary equivalent
    pub model: String,
    pub created_at_ms: u64,         // Unix timestamp millis
    pub hit_count: u32,
}

pub struct CacheEntry {
    pub key: String,
    pub value: CachedResponse,
}

pub struct ResponseCache {
    db_path: PathBuf,              // ~/.elai/cache.json
    ttl_ms: u64,                   // default 3_600_000 (1h)
    enabled: bool,
    entries: BTreeMap<String, CachedResponse>,  // lazy-loaded
    dirty: bool,                   // track whether flush is needed
}
```

**Public API:**

```rust
impl ResponseCache {
    pub fn new(db_path: PathBuf, ttl_ms: u64) -> Self;
    pub fn disabled() -> Self;  // for --no-cache
    pub fn get(&mut self, key: &CacheKey) -> Option<&CachedResponse>;
    pub fn put(&mut self, key: CacheKey, response: CachedResponse);
    pub fn evict_expired(&mut self) -> usize;
    pub fn clear(&mut self);
    pub fn stats(&self) -> CacheStats;
    pub fn flush(&self) -> Result<(), io::Error>;  // persist to disk
}
```

**Key generation function:**

```rust
pub fn generate_cache_key(
    messages: &[ConversationMessage],
    model: &str,
    system_prompt: &[String],
) -> Option<CacheKey>;
// Returns None if ANY message contains ToolUse or ToolResult blocks (invariant enforcement)
```

Uses the existing `json.rs` `JsonValue::Object(BTreeMap)` which already sorts keys, then `sha2::Sha256` (already a dependency) for the hash.

**Tool-call bypass logic:**

```rust
fn contains_tool_content(messages: &[ConversationMessage]) -> bool {
    messages.iter().any(|msg| {
        msg.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
        ))
    })
}
```

**Acceptance criteria:**
- [x] `CacheKey` is a newtype over `String` containing a 64-char hex SHA-256
- [x] `generate_cache_key` returns `None` when any message has tool content
- [x] `get` returns `None` for expired entries and increments `hit_count` on hit
- [x] `put` is a no-op when `enabled == false`
- [x] `flush` writes sorted JSON to `~/.elai/cache.json`
- [x] `evict_expired` removes entries older than TTL

---

### Step 2: Export from `crates/runtime/src/lib.rs`

**Modified file:** `crates/runtime/src/lib.rs`

Add:
```rust
mod response_cache;
pub use response_cache::{
    CacheKey, CacheStats, CachedResponse, ResponseCache, generate_cache_key,
};
```

**Acceptance criteria:**
- [x] Module compiles and is re-exported
- [x] No clippy warnings

---

### Step 3: Wire Cache into CLI Request Pipeline

**Modified file:** `crates/elai-cli/src/app.rs`

The interception point is `render_response()` (line 309), which calls `self.conversation_client.run_turn()`.

Add `ResponseCache` as a field on `CliApp`:

```rust
pub struct CliApp {
    // existing fields...
    cache: ResponseCache,
}
```

In `render_response`, before calling `run_turn`:

```
1. Build cache key from conversation_history + input + model + system_prompt
2. If key is Some (no tool content):
   a. Check cache.get(&key)
   b. If hit: deserialize cached response, skip API call, render directly
   c. If miss: proceed with API call, then cache.put() after success
3. If key is None (tool content present): skip cache entirely
4. On Drop / session end: cache.flush()
```

**Important:** The cached path must still produce the same output format (Text/Json/Ndjson) as the live path. The cache stores the raw assistant text + usage, not rendered output.

**Acceptance criteria:**
- [x] Cache hit skips the API call entirely
- [x] Cache miss proceeds normally and stores the result
- [x] Tool-bearing conversations bypass cache at key-generation time
- [x] `cache.flush()` is called when the REPL exits
- [x] Cached responses render identically to live responses

---

### Step 4: Add `--no-cache` Flag and `/cache` Slash Command

**Modified file:** `crates/elai-cli/src/args.rs`

```rust
#[arg(long)]
pub no_cache: bool,
```

**Modified file:** `crates/elai-cli/src/app.rs`

Add to `SlashCommand` enum:
```rust
pub enum SlashCommand {
    Help,
    Status,
    Compact,
    CacheClear,   // NEW
    CacheStats,   // NEW
    Unknown(String),
}
```

Parse `/cache clear` and `/cache stats`:
```rust
"cache" => {
    let subcommand = parts.next().unwrap_or("stats");
    match subcommand {
        "clear" => Self::CacheClear,
        "stats" | "" => Self::CacheStats,
        _ => Self::Unknown(format!("cache {subcommand}")),
    }
}
```

Handler implementations:
- `/cache clear` -- calls `self.cache.clear()` + `self.cache.flush()`, prints "Cache cleared."
- `/cache stats` -- calls `self.cache.stats()`, prints entry count, total hits, oldest entry age

Add to `SLASH_COMMAND_HANDLERS` and update `handle_help`.

When `--no-cache` is set, initialize `ResponseCache::disabled()` instead of the normal constructor.

**Acceptance criteria:**
- [x] `--no-cache` flag prevents all cache reads and writes
- [x] `/cache clear` empties the cache file
- [x] `/cache stats` shows entry count, hit count, oldest entry
- [x] `/help` lists the new cache commands

---

### Step 5: Tests

**Location:** `crates/runtime/src/response_cache.rs` (inline `#[cfg(test)]` module)

Required test cases:

1. **Deterministic key generation**: Same messages + model + system produce identical `CacheKey` across calls
2. **Key ordering independence**: Messages serialized with `BTreeMap` produce same hash regardless of field insertion order (inherently true via `BTreeMap`, but verify)
3. **Tool-call bypass (input)**: `generate_cache_key` returns `None` when messages contain `ToolUse`
4. **Tool-call bypass (result)**: `generate_cache_key` returns `None` when messages contain `ToolResult`
5. **TTL expiration**: Entry inserted with `created_at_ms` in the past is not returned by `get`
6. **Hit counter**: `get` increments `hit_count` on cache hit
7. **Flush and reload**: `flush` writes JSON, new `ResponseCache` loading from same path recovers entries
8. **Evict expired**: `evict_expired` removes stale entries and returns count
9. **Disabled mode**: `ResponseCache::disabled()` returns `None` on `get`, no-ops on `put`

**Acceptance criteria:**
- [x] All 9 test cases pass
- [x] Tests use `tempdir` for cache file isolation
- [x] No test depends on real time (inject `created_at_ms` directly)

---

## File Summary

| File | Action | Description |
|---|---|---|
| `crates/runtime/src/response_cache.rs` | CREATE | Core cache module: CacheKey, ResponseCache, generate_cache_key |
| `crates/runtime/src/lib.rs` | MODIFY | Add `mod response_cache` + re-exports |
| `crates/elai-cli/src/app.rs` | MODIFY | Wire cache into CliApp, add /cache slash commands |
| `crates/elai-cli/src/args.rs` | MODIFY | Add `--no-cache` flag |

---

## Dependencies

- **No new crate dependencies.** `sha2` is already in `runtime/Cargo.toml`. `BTreeMap` from std provides sorted-key serialization.
- If file locking is desired for safety: `fs2` crate (optional, ~15 LOC, pure Rust). Not required for v1 single-process CLI.

---

## Success Criteria

1. `cargo test -p runtime` passes with all 9 new cache tests green
2. `cargo clippy --workspace` passes with zero warnings
3. Running the same prompt twice in the REPL results in an instant response on the second call (cache hit)
4. Adding a tool call to the conversation causes subsequent requests to bypass cache
5. `--no-cache` flag prevents all caching
6. `/cache clear` empties `~/.elai/cache.json`
7. Cache file is valid JSON that can be inspected manually

---

## Upgrade Path (Future)

When the JSON file approach shows limitations:
1. Add `rusqlite` to `runtime/Cargo.toml` behind a `sqlite-cache` feature flag
2. Implement `SqliteResponseCache` with the same public API
3. Swap the concrete type in `CliApp::new` based on feature flag
4. Migration: read existing JSON cache, insert into SQLite, delete JSON file
