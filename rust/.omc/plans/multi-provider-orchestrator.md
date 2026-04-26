# Multi-Provider Orchestrator with Intelligent Fallback

**Date:** 2026-04-26
**Complexity:** HIGH
**Scope:** ~12 files to create/modify across `crates/api/`
**Reference:** mythos-router `orchestrator.ts` + `types.ts`

---

## Context & Analysis of Current Architecture

### What exists today

The `crates/api/` crate has a clean provider abstraction:

- **`Provider` trait** (`providers/mod.rs:12-24`): Generic trait with associated `Stream` type, `send_message` and `stream_message` methods returning `ProviderFuture<T>`.
- **`ProviderClient` enum** (`client.rs:22-26`): Dispatch enum with variants `ClawApi(ClawApiClient)`, `Xai(OpenAiCompatClient)`, `OpenAi(OpenAiCompatClient)`. Routes by model name to the correct backend.
- **`ClawApiClient`** (`claw_provider.rs`): Anthropic API client with built-in retry (exponential backoff, max 2 retries), OAuth support, SSE streaming.
- **`OpenAiCompatClient`** (`openai_compat.rs`): OpenAI-compatible client (xAI/OpenAI) with same retry pattern.
- **`DefaultRuntimeClient`** (`claw-cli/src/main.rs:3547-3558`): Wraps `ProviderClient` + a Tokio runtime. Implements the `ApiClient` trait (sync `stream(&mut self, request) -> Vec<AssistantEvent>`) which is what `ConversationRuntime<C, T>` consumes.

### Key architectural constraints

1. **`DefaultRuntimeClient` owns a single `ProviderClient`** -- selected once at startup from the model name. No fallback.
2. **`Provider` trait has an associated `Stream` type** -- this prevents object-safe usage (`dyn Provider`). The `ProviderClient` enum already works around this with a `MessageStream` dispatch enum.
3. **`ConversationRuntime` is generic over `C: ApiClient`** -- the `ApiClient` trait is synchronous (`&mut self`). The orchestrator must present the same interface.
4. **Tokio runtime is created per-client** in `DefaultRuntimeClient`, not shared.

---

## Work Objectives

Port the mythos-router orchestration pattern (EMA metrics, circuit breaker, deterministic routing, automatic fallback) into claw's Rust crate, respecting Rust's ownership/concurrency model.

---

## Guardrails

### MUST HAVE
- Backward compatible: single-provider mode must work exactly as today
- All existing tests must pass
- New code must be `Send + Sync` safe for Tokio async
- Circuit breaker cooldown of 5 minutes
- Fallback on status codes 429, 502, 503, 504, and timeouts
- EMA smoothing factor alpha = 0.3

### MUST NOT
- Break the existing `Provider` trait contract
- Require changes to `ConversationRuntime` generic signature
- Add runtime overhead when only one provider is configured
- Use `unsafe` code
- Block the Tokio runtime (no `std::thread::sleep` in async context)

---

## Task Flow (Implementation Order)

### Step 1: Orchestrator Types & Metrics Module

**Create** `crates/api/src/orchestrator/mod.rs`
**Create** `crates/api/src/orchestrator/metrics.rs`
**Create** `crates/api/src/orchestrator/types.rs`

#### types.rs -- Core data structures

```rust
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderCapability {
    Thinking,
    ToolCalling,
    Streaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatus {
    Healthy,
    Degraded,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    Chat,
    Code,
    Analysis,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    RateLimit,
    Timeout,
    ServerError,
    NetworkError,
    CapabilityMismatch,
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: String,
    pub priority: usize,         // lower = higher priority
    pub enabled: bool,
    pub max_concurrency: usize,
}

#[derive(Debug, Clone)]
pub struct OrchestrationEvent {
    pub timestamp: std::time::Instant,
    pub session_id: String,
    pub command: String,          // "stream" | "send"
    pub primary_provider: String,
    pub actual_provider: String,
    pub fallback_reason: Option<FallbackReason>,
    pub latency_ms: u64,
    pub retry_count: u32,
}

pub struct RequestOptions {
    pub task_type: TaskType,
    pub deterministic: bool,
}
```

#### metrics.rs -- EMA metrics + scoring

```rust
const EMA_ALPHA: f64 = 0.3;

#[derive(Debug, Clone)]
pub struct ModelMetrics {
    pub success_rate: f64,       // EMA of success (0.0 - 1.0)
    pub avg_latency_ms: f64,     // EMA of latency
    pub prev_success_rate: f64,
    pub prev_avg_latency_ms: f64,
    pub cost_per_1k: f64,
    pub total_calls: u64,
    pub total_failures: u64,
    pub last_error: Option<String>,
    pub last_error_time: Option<std::time::Instant>,
}

impl ModelMetrics {
    pub fn new() -> Self { /* defaults: success_rate=1.0, avg_latency=1000.0 */ }
    pub fn record_success(&mut self, latency_ms: f64, cost: f64) { /* EMA update */ }
    pub fn record_failure(&mut self, error: &str) { /* EMA update */ }
    pub fn score(&self, task_type: TaskType) -> f64 { /* weighted formula */ }
}
```

**Acceptance criteria:**
- `ModelMetrics::record_success` applies EMA correctly: `new = old * (1 - 0.3) + sample * 0.3`
- `ModelMetrics::score` returns higher values for better providers
- All types derive necessary traits (`Debug`, `Clone`, `Send`, `Sync`)
- Unit tests for EMA convergence and score ordering

---

### Step 2: `UnifiedProvider` Trait + Provider Slot

**Create** `crates/api/src/orchestrator/provider.rs`

```rust
use async_trait::async_trait;
use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse};
use super::types::ProviderCapability;
use std::collections::HashSet;

/// Object-safe provider interface for the orchestrator.
/// Unlike the existing `Provider` trait (which has an associated Stream type),
/// this trait erases the stream type to allow heterogeneous provider collections.
#[async_trait]
pub trait UnifiedProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> &HashSet<ProviderCapability>;

    async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError>;

    /// Stream message, collecting events into a final MessageResponse.
    /// The orchestrator handles fallback at the response level, not mid-stream.
    async fn stream_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError>;

    /// Lightweight health probe (e.g., HEAD request or cached status).
    async fn health_check(&self) -> Result<(), ApiError>;
}
```

**Adapter implementations** -- wrap existing clients:

```rust
pub struct ClawUnifiedAdapter {
    client: ClawApiClient,
    capabilities: HashSet<ProviderCapability>,
}

pub struct OpenAiUnifiedAdapter {
    client: OpenAiCompatClient,
    provider_id: String,
    capabilities: HashSet<ProviderCapability>,
}
```

Both implement `UnifiedProvider` by delegating to their inner client. For `stream_message`, consume the `MessageStream` fully and reconstruct a `MessageResponse` (the orchestrator only does response-level fallback; actual streaming passthrough is handled in Step 4).

**Provider Slot** -- runtime state per provider:

```rust
pub(crate) struct ProviderSlot {
    pub provider: Box<dyn UnifiedProvider>,
    pub config: ProviderConfig,
    pub status: ProviderStatus,
    pub metrics: ModelMetrics,
    pub active_concurrency: AtomicUsize,  // lock-free counter
    pub degraded_until: AtomicU64,        // unix timestamp ms, 0 = not degraded
}
```

**Acceptance criteria:**
- `UnifiedProvider` is object-safe (`dyn UnifiedProvider` compiles)
- Adapters pass `Send + Sync` bounds
- `ProviderSlot` uses `AtomicUsize`/`AtomicU64` for concurrency and degraded_until (no Mutex needed for these hot-path counters)
- Unit test: `ClawUnifiedAdapter` correctly delegates to underlying client mock

---

### Step 3: `ProviderOrchestrator` Core Logic

**Create** `crates/api/src/orchestrator/orchestrator.rs`

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use sha2::{Sha256, Digest};

const CIRCUIT_BREAKER_COOLDOWN_MS: u64 = 5 * 60 * 1000;
const RETRY_BACKOFFS_MS: &[u64] = &[100, 500, 1000];
const DEFAULT_WATCHDOG_MS: u64 = 15_000;
const WATCHDOG_LATENCY_MULTIPLIER: f64 = 3.0;

pub struct ProviderOrchestrator {
    slots: Vec<Arc<RwLock<ProviderSlot>>>,   // RwLock: reads concurrent, writes exclusive
    event_log: Arc<RwLock<VecDeque<OrchestrationEvent>>>,
    session_id: String,
}
```

**Key methods:**

| Method | Description |
|--------|-------------|
| `register_provider(provider, config)` | Adds a `ProviderSlot` to `slots` |
| `select_providers(options) -> Vec<Arc<RwLock<ProviderSlot>>>` | Filters eligible providers, resets expired circuit breakers, sorts by score (or returns all for deterministic mode) |
| `deterministic_select(messages, eligible) -> usize` | SHA-256 hash of message content modulo eligible count |
| `send_message(request, options) -> Result<MessageResponse, ApiError>` | Iterates candidates with retry-per-provider, records metrics, returns first success |
| `stream_message(request, options) -> Result<OrchestratedStream, ApiError>` | Same fallback logic but returns a stream wrapper (see Step 4) |
| `record_success(slot, latency_ms, cost)` | Updates EMA metrics |
| `record_failure(slot, error)` | Updates EMA, may trip circuit breaker |
| `trip_circuit_breaker(slot)` | Sets `degraded_until = now + 5min` |
| `provider_health() -> Vec<HealthReport>` | Public observability |

**Concurrency design (CRITICAL for Rust):**

```
Orchestrator
  |-- slots: Vec<Arc<RwLock<ProviderSlot>>>
       |-- provider: Box<dyn UnifiedProvider>   (immutable after registration)
       |-- metrics: ModelMetrics                (mutated via write lock)
       |-- active_concurrency: AtomicUsize      (lock-free CAS)
       |-- degraded_until: AtomicU64            (lock-free store/load)
```

- **Read path** (select_providers): Acquire `RwLock::read()` on each slot. Multiple concurrent reads allowed.
- **Write path** (record_success/failure): Acquire `RwLock::write()` only on the specific slot being updated. No global lock.
- **Concurrency counter**: Use `AtomicUsize::fetch_add` / `fetch_sub` -- no lock needed.
- **Circuit breaker timestamp**: Use `AtomicU64::store` / `load` with `Ordering::Relaxed` (eventual consistency is acceptable).

**Deterministic selection:**

```rust
fn deterministic_select(messages: &[InputMessage], count: usize) -> usize {
    let mut hasher = Sha256::new();
    for msg in messages {
        hasher.update(msg.role.as_bytes());
        hasher.update(b":");
        // hash content blocks
    }
    let hash = hasher.finalize();
    let index = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
    (index as usize) % count
}
```

**Retryable error detection** (reuse existing `ApiError::is_retryable`):

```rust
fn is_retryable(error: &ApiError) -> bool {
    match error {
        ApiError::Api { status, .. } => {
            matches!(status.as_u16(), 429 | 502 | 503 | 504)
        }
        ApiError::Http(_) => true,  // network errors
        _ => error.is_retryable(),
    }
}

fn extract_fallback_reason(error: &ApiError) -> FallbackReason {
    match error {
        ApiError::Api { status, .. } if status.as_u16() == 429 => FallbackReason::RateLimit,
        ApiError::Http(e) if e.is_timeout() => FallbackReason::Timeout,
        ApiError::Http(_) => FallbackReason::NetworkError,
        _ => FallbackReason::ServerError,
    }
}
```

**Acceptance criteria:**
- Orchestrator is `Send + Sync` (required for Tokio `.await` across boundaries)
- Circuit breaker trips after retry exhaustion and resets after 5 minutes
- Deterministic mode: same input always selects same provider (given same eligible set)
- Adaptive mode: higher-scored providers are tried first
- All providers exhausted returns `ApiError` (not panic)
- Unit tests with mock providers: test fallback sequence, circuit breaker trip/reset, deterministic routing stability

---

### Step 4: Streaming Passthrough with Fallback

**Create** `crates/api/src/orchestrator/stream.rs`

The challenge: the current `MessageStream` emits `StreamEvent` chunks. Orchestrator fallback needs to detect failure and switch providers. Two strategies:

**Strategy chosen: Response-level fallback with stream wrapping**

```rust
pub enum OrchestratedStream {
    /// Direct passthrough -- no fallback needed mid-stream
    Direct(client::MessageStream),
    /// Collected response re-emitted as synthetic stream events
    Collected(CollectedStream),
}

impl OrchestratedStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Direct(s) => s.next_event().await,
            Self::Collected(s) => s.next_event(),
        }
    }
}
```

For the primary (first-choice) provider: pass through the raw stream directly.
If the primary fails before producing any events, fall back to next provider. The fallback provider's response is collected fully and then re-emitted as synthetic stream events.

This avoids the complexity of mid-stream provider switching while still providing fallback.

**Acceptance criteria:**
- `OrchestratedStream` implements the same `next_event` interface as `MessageStream`
- Primary provider streams directly (zero overhead)
- Fallback providers' responses are emitted as synthetic `StreamEvent` sequence
- Test: simulate primary failure, verify fallback stream produces correct events

---

### Step 5: Integration into `ProviderClient` & `DefaultRuntimeClient`

**Modify** `crates/api/src/client.rs`

Add orchestrator-aware construction:

```rust
pub enum ProviderClient {
    // Existing single-provider variants (unchanged)
    ClawApi(ClawApiClient),
    Xai(OpenAiCompatClient),
    OpenAi(OpenAiCompatClient),
    // New: orchestrated multi-provider
    Orchestrated(Arc<ProviderOrchestrator>),
}
```

Add factory method:

```rust
impl ProviderClient {
    /// Create an orchestrated client with all available providers.
    /// Falls back to single-provider mode if only one key is configured.
    pub fn orchestrated() -> Result<Self, ApiError> {
        let mut orchestrator = ProviderOrchestrator::new();
        let mut count = 0;

        // Register Anthropic if key available
        if let Ok(client) = ClawApiClient::from_env() {
            orchestrator.register_provider(
                Box::new(ClawUnifiedAdapter::new(client)),
                ProviderConfig { id: "anthropic".into(), priority: 0, enabled: true, max_concurrency: 3 },
            );
            count += 1;
        }
        // Register OpenAI if key available
        if let Ok(client) = OpenAiCompatClient::from_env(OpenAiCompatConfig::openai()) {
            orchestrator.register_provider(
                Box::new(OpenAiUnifiedAdapter::new(client, "openai")),
                ProviderConfig { id: "openai".into(), priority: 1, enabled: true, max_concurrency: 3 },
            );
            count += 1;
        }
        // Register xAI if key available
        if let Ok(client) = OpenAiCompatClient::from_env(OpenAiCompatConfig::xai()) {
            orchestrator.register_provider(
                Box::new(OpenAiUnifiedAdapter::new(client, "xai")),
                ProviderConfig { id: "xai".into(), priority: 2, enabled: true, max_concurrency: 3 },
            );
            count += 1;
        }

        if count == 0 {
            return Err(ApiError::missing_credentials("any provider", &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "XAI_API_KEY"]));
        }

        Ok(Self::Orchestrated(Arc::new(orchestrator)))
    }
}
```

**Modify** `crates/claw-cli/src/main.rs` -- `DefaultRuntimeClient::new`:

```rust
// When --orchestrate flag is passed (or env CLAW_ORCHESTRATE=1):
let client = if orchestrate_mode {
    ProviderClient::orchestrated()?
} else {
    ProviderClient::from_model(&model)?
};
```

**Modify** `crates/api/src/lib.rs` -- export new public types.

**Acceptance criteria:**
- `ProviderClient::Orchestrated` variant handles `send_message` and `stream_message` by delegating to `ProviderOrchestrator`
- Single-provider path is unchanged (no performance regression)
- CLI flag `--orchestrate` or env `CLAW_ORCHESTRATE=1` enables multi-provider mode
- Existing tests pass without modification

---

### Step 6: Metrics Persistence + Tests

**Create** `crates/api/src/orchestrator/persistence.rs`

```rust
use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct PersistedMetrics {
    version: u32,
    providers: Vec<PersistedProviderMetrics>,
}

#[derive(Serialize, Deserialize)]
struct PersistedProviderMetrics {
    id: String,
    success_rate: f64,
    avg_latency_ms: f64,
    total_calls: u64,
    total_failures: u64,
}

fn metrics_path() -> PathBuf {
    // ~/.config/claw/orchestrator-metrics.json
    let config_dir = std::env::var("CLAW_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::config_dir().unwrap().join("claw"));
    config_dir.join("orchestrator-metrics.json")
}

pub fn save_metrics(metrics: &[(&str, &ModelMetrics)]) -> Result<(), std::io::Error> { /* ... */ }
pub fn load_metrics() -> Result<Option<PersistedMetrics>, std::io::Error> { /* ... */ }
```

Load at orchestrator construction; save periodically (every N calls or on session end).

**Integration tests** -- Create `crates/api/tests/orchestrator_integration.rs`:

1. **Test EMA convergence**: Feed 100 successes at 200ms, verify avg_latency converges near 200ms
2. **Test circuit breaker**: Feed 4 consecutive failures, verify status becomes `Degraded`, verify recovery after 5min (mocked time)
3. **Test deterministic routing**: Same messages always select same provider index
4. **Test fallback chain**: Mock 2 providers, first always fails, verify second is used and metrics reflect correctly
5. **Test single-provider no-overhead**: Verify `Orchestrated` with 1 provider degrades to direct call

**Acceptance criteria:**
- Metrics file round-trips correctly (save then load produces identical data)
- Metrics persist across CLI invocations
- All 5 integration test scenarios pass
- `cargo test` passes for the entire workspace

---

## Files Summary

### New files (7)

| File | Purpose |
|------|---------|
| `crates/api/src/orchestrator/mod.rs` | Module root, re-exports |
| `crates/api/src/orchestrator/types.rs` | Enums, configs, events |
| `crates/api/src/orchestrator/metrics.rs` | EMA metrics + scoring |
| `crates/api/src/orchestrator/provider.rs` | `UnifiedProvider` trait + adapters |
| `crates/api/src/orchestrator/orchestrator.rs` | Core orchestration logic |
| `crates/api/src/orchestrator/stream.rs` | `OrchestratedStream` wrapper |
| `crates/api/src/orchestrator/persistence.rs` | JSON metrics persistence |

### Modified files (5)

| File | Change |
|------|--------|
| `crates/api/src/lib.rs` | Add `pub mod orchestrator`, export key types |
| `crates/api/src/client.rs` | Add `Orchestrated` variant to `ProviderClient` enum |
| `crates/api/Cargo.toml` | Add deps: `sha2`, `async-trait`, `dirs` |
| `crates/claw-cli/src/main.rs` | Add `--orchestrate` flag, wire into `DefaultRuntimeClient` |
| `crates/api/tests/orchestrator_integration.rs` | New integration test file |

### New dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `sha2` | `0.10` | Deterministic provider selection hash |
| `async-trait` | `0.1` | Object-safe async trait for `UnifiedProvider` |
| `dirs` | `5` | Config directory resolution (may already be transitive) |

---

## Concurrency Pitfalls & Mitigations

| Pitfall | Mitigation |
|---------|------------|
| **`RwLock` starvation**: Many reads block writes | Keep write locks extremely short (only metric updates). Consider `parking_lot::RwLock` for fairness. |
| **`Arc<RwLock<ProviderSlot>>` in async**: Must not hold lock across `.await` | Acquire lock, copy/update data, drop lock, then await. Never `let _guard = slot.write().await; do_network().await`. |
| **Atomic ordering on `degraded_until`**: Relaxed is fine? | Yes. Worst case: one extra request sent to a just-tripped provider. Acceptable. |
| **`Box<dyn UnifiedProvider>` inside `RwLock`**: Provider itself is immutable after registration | The `provider` field is read-only. Only `metrics`, `active_concurrency`, `degraded_until` are mutated. Consider splitting into `ProviderSlotImmutable` + `ProviderSlotState` to allow `RwLock` only on state. |
| **`tokio::time::sleep` in retry backoff**: Must use Tokio sleep, not `std::thread::sleep` | All retry logic is async. Use `tokio::time::sleep`. |
| **Provider registration after startup**: Not thread-safe if slots Vec grows | Register all providers at construction time. `slots` is immutable after `ProviderOrchestrator::new()` returns. |

---

## Success Criteria

1. `cargo test --workspace` passes with zero failures
2. `cargo clippy --workspace` passes with zero warnings
3. Single-provider mode (no `--orchestrate`) works identically to current behavior
4. Multi-provider mode correctly falls back when primary provider returns 429/502/503
5. Circuit breaker prevents repeated calls to failing provider for 5 minutes
6. Same input deterministically routes to same provider
7. Metrics persist across CLI sessions via `~/.config/claw/orchestrator-metrics.json`
8. `ProviderOrchestrator` is `Send + Sync`
