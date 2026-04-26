# SWD Correction Turns — Implementation Plan

**Date:** 2026-04-26
**Scope:** 6 files modified, 1 new struct, ~300 lines added
**Complexity:** MEDIUM
**Reference:** `mythos-router/src/commands/chat.ts` lines 318-389

---

## Context

The SWD (Strict Write Discipline) engine currently detects failures (`SwdOutcome::Failed`, `RolledBack`, `Drift`) and logs them, but **takes no corrective action**. After a failed SWD verification the system simply records the outcome and moves on, leaving broken writes in place (partial mode rolls back, full mode logs the failure). The model never learns that its write failed.

The TypeScript reference implements a correction loop (`runCorrectionLoop`) that:
1. Filters failed/drift results into a structured prompt
2. Sends the prompt as a user message back to the model
3. Re-executes the model's corrected output through SWD
4. Caps retries at `MAX_CORRECTION_RETRIES` (2)

---

## Current Architecture (What Happens Today)

### Partial mode (`SwdLevel::Partial`)
- `CliToolExecutor::execute()` intercepts write tools (line 4482-4485 in main.rs)
- Calls `execute_with_swd()` which: snapshots before, executes tool, snapshots after, verifies, rolls back on failure
- Returns the `SwdTransaction` via `TuiMsg::SwdResult` to the TUI
- **After failure: nothing. The tool returns an error to the runtime, the model sees it as a tool error, but no structured SWD feedback is provided**

### Full mode (`SwdLevel::Full`)
- Write tools are blocked; model emits `[FILE_ACTION]` blocks in text
- On `MessageStop` (line 3738-3748 in main.rs), `parse_file_actions()` + `execute_file_actions()` runs
- Transactions are sent via `TuiMsg::SwdBatchResult`
- **After failure: nothing. The failed transactions are logged but the conversation ends**

### The agentic loop (`runtime/src/conversation.rs` line 153-258)
- `run_turn()` pushes user message, then loops: stream -> build assistant message -> execute tools -> push tool results -> loop if more tool uses
- Loop breaks when the assistant produces no tool uses
- **This is where correction turns must be injected for partial mode** (tool results feed back naturally)
- **For full mode, correction must happen AFTER the loop exits** (since FILE_ACTIONs are in text, not tool calls)

---

## Work Objectives

Implement an SWD correction turn mechanism that:
- Detects failed/drift SWD transactions after each turn
- Constructs a structured feedback message and re-sends to the model
- Caps retries at 2 per user turn (configurable via `CorrectionContext`)
- Works for both partial and full SWD modes
- Shows retry status in the TUI
- Resets the attempt counter on each new user turn

---

## Guardrails

### Must Have
- Maximum 2 correction attempts per user turn (hard cap, no config override above 3)
- Structured failure feedback with: path, operation, before_hash, after_hash, failure reason
- Counter resets on new user input
- TUI indicator: `"SWD retry 1/2"` on correction turn
- Correction prompt is injected as a user-role message in the conversation history
- Failed correction after max attempts logs warning and yields control to user

### Must NOT Have
- Infinite retry loops (max_attempts is enforced at the struct level with a const cap)
- Architecture changes to `runtime/src/conversation.rs` `run_turn()` loop itself (inject at call sites)
- Budget/token tracking in this phase (can be added later, unlike the TS version)
- Changes to the SWD engine (`swd.rs`) verification logic itself
- Correction attempts for `SwdOutcome::Noop` or `SwdOutcome::Verified`

---

## Task Flow

```
User message
    |
    v
run_turn() executes
    |
    v
[Partial mode]                    [Full mode]
Tool executions produce           MessageStop triggers
SwdTransactions via               execute_file_actions()
execute_with_swd()                producing Vec<SwdTransaction>
    |                                 |
    v                                 v
    +----------> has_swd_failures() <-+
                      |
                 yes  |  no --> done
                      v
              correction_ctx.attempts < max?
                 yes  |  no --> yield to human
                      v
              build_correction_prompt(failures)
                      |
                      v
              inject as user message
              call run_turn() / re-stream
                      |
                      v
              check new SWD results
              loop back to has_swd_failures()
```

---

## Detailed TODOs

### Step 1: Add `CorrectionContext` struct to `swd.rs`

**File:** `crates/claw-cli/src/swd.rs`
**Location:** After the `SwdTransaction` struct (line ~98)

```rust
pub const MAX_CORRECTION_ATTEMPTS: u8 = 2;

#[derive(Debug, Clone)]
pub struct CorrectionContext {
    pub attempts: u8,
    pub max_attempts: u8,
    pub last_failures: Vec<SwdTransaction>,
}

impl CorrectionContext {
    pub fn new() -> Self {
        Self {
            attempts: 0,
            max_attempts: MAX_CORRECTION_ATTEMPTS,
            last_failures: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
        self.last_failures.clear();
    }

    pub fn can_retry(&self) -> bool {
        self.attempts < self.max_attempts
    }

    pub fn record_failures(&mut self, txs: &[SwdTransaction]) {
        self.attempts += 1;
        self.last_failures = txs
            .iter()
            .filter(|tx| matches!(tx.outcome, SwdOutcome::Failed { .. } | SwdOutcome::Drift { .. } | SwdOutcome::RolledBack))
            .cloned()
            .collect();
    }

    pub fn has_failures(&self) -> bool {
        !self.last_failures.is_empty()
    }
}
```

Also add a helper function:

```rust
pub fn build_correction_prompt(failures: &[SwdTransaction]) -> String {
    let mut lines = vec!["[SWD CORRECTION TURN]".to_string()];
    lines.push("File actions failed verification:".to_string());
    for tx in failures {
        let status = tx.outcome.as_str().to_uppercase();
        let detail = match &tx.outcome {
            SwdOutcome::Failed { reason } => reason.clone(),
            SwdOutcome::Drift { detail } => detail.clone(),
            SwdOutcome::RolledBack => "rolled back after batch failure".to_string(),
            _ => String::new(),
        };
        lines.push(format!(
            "- [{status}] {tool} {path}: {detail} (before={before}, after={after})",
            tool = tx.tool_name,
            path = tx.path,
            before = tx.before_hash.as_deref().unwrap_or("none"),
            after = tx.after_hash.as_deref().unwrap_or("none"),
        ));
    }
    lines.push(String::new());
    lines.push("Please correct your response and retry the failed file operations.".to_string());
    lines.join("\n")
}
```

**Acceptance criteria:**
- `CorrectionContext::new()` starts with `attempts=0`, `max_attempts=2`
- `can_retry()` returns `true` when `attempts < max_attempts`
- `record_failures()` increments attempts and filters only failed/drift/rolledback transactions
- `build_correction_prompt()` produces a structured string with all failure details
- `reset()` zeroes the counter and clears failures

---

### Step 2: Add `TuiMsg::CorrectionRetry` variant and TUI rendering

**File:** `crates/claw-cli/src/tui.rs`

**2a.** Add new variant to `TuiMsg` enum (line ~62):
```rust
CorrectionRetry { attempt: u8, max_attempts: u8 },
```

**2b.** Add new variant to `ChatEntry` enum (line ~90):
```rust
CorrectionRetryEntry { attempt: u8, max_attempts: u8 },
```

**2c.** Handle in `TuiApp::handle_msg()` (after `SwdBatchResult` handler, line ~253):
```rust
TuiMsg::CorrectionRetry { attempt, max_attempts } => {
    self.push_chat(ChatEntry::CorrectionRetryEntry { attempt, max_attempts });
}
```

**2d.** Render in `build_chat_lines()` (in the `ChatEntry` match, before `SwdLogEntry`, line ~1319):
```rust
ChatEntry::CorrectionRetryEntry { attempt, max_attempts } => {
    result.push(Line::from(Span::styled(
        format!("  ↩ SWD retry {attempt}/{max_attempts}"),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    result.push(Line::from(""));
}
```

**Acceptance criteria:**
- TUI displays `"  ↩ SWD retry 1/2"` in yellow bold when a correction turn fires
- The entry appears in the chat log between the failed SWD log and the model's correction response

---

### Step 3: Wire correction loop into the TUI background thread (Full mode)

**File:** `crates/claw-cli/src/main.rs`
**Location:** Inside `DefaultRuntimeClient::stream()`, the `MessageStop` handler (lines 3732-3749)

Currently the code executes file actions and sends `SwdBatchResult`. Modify to:

1. After `execute_file_actions()`, check if any transaction has Failed/Drift/RolledBack status
2. If yes and `CorrectionContext::can_retry()`:
   - Send `TuiMsg::CorrectionRetry { attempt, max_attempts }` to TUI
   - Build correction prompt via `build_correction_prompt()`
   - Append the correction prompt as a user message to the request messages
   - Append the current assistant text as an assistant message
   - Re-call `self.client.stream_message()` with the updated messages
   - Process the new stream (reuse the same streaming logic)
   - Parse new `FILE_ACTION` blocks from the new response
   - Execute and verify again
   - Loop until success or max attempts exhausted

**Implementation approach:** Extract the `MessageStop` SWD handling into a helper method on `DefaultRuntimeClient`:

```rust
fn handle_full_swd_correction(
    &self,
    runtime: &tokio::runtime::Runtime,
    full_text: &str,
    request_messages: &mut Vec<ConversationMessage>,
    tui_sender: &Option<mpsc::Sender<tui::TuiMsg>>,
    correction_ctx: &mut CorrectionContext,
) -> Vec<AssistantEvent> { ... }
```

This method contains the retry loop internally, capped by `correction_ctx.can_retry()`.

**Where `CorrectionContext` lives:** Add it as a field on `DefaultRuntimeClient`. It is `reset()` at the start of each `stream()` call (which corresponds to one model turn).

**IMPORTANT — preventing infinite loops:**
- `CorrectionContext.attempts` is incremented BEFORE each retry call
- `can_retry()` is checked BEFORE incrementing
- The const `MAX_CORRECTION_ATTEMPTS = 2` means at most 2 retries (3 total attempts including original)
- If the correction stream itself panics or errors, the loop breaks immediately
- `correction_ctx.reset()` is called at the top of `stream()` so each user turn starts fresh

**Acceptance criteria:**
- Full mode: after a failed `execute_file_actions()`, the model receives a correction prompt and retries
- After 2 failed retries, the loop stops and logs "Max corrections reached"
- The corrected response's FILE_ACTIONs are executed through the same SWD pipeline
- Each retry sends `TuiMsg::CorrectionRetry` before the new stream
- New `SwdBatchResult` messages are sent for each retry's results

---

### Step 4: Wire correction feedback into partial mode (tool-level)

**File:** `crates/claw-cli/src/main.rs`
**Location:** `CliToolExecutor::execute_with_swd()` (lines 4378-4449)

For partial mode, the correction happens at the tool level. When `execute_with_swd()` detects a failure:

1. Instead of just returning the error, enrich the tool error message with SWD correction context:

```rust
// After rollback (line 4411), before building tx_record:
if matches!(outcome, SwdOutcome::Failed { .. } | SwdOutcome::Drift { .. }) {
    let correction_hint = format!(
        "SWD verification failed for {path}:\n\
         - Before hash: {before}\n\
         - After hash: {after}\n\
         - Reason: {reason}\n\
         The file has been rolled back. Please retry with corrected content.",
        path = path,
        before = before_hash.as_deref().unwrap_or("none"),
        after = after_hash.as_deref().unwrap_or("none"),
        reason = match &outcome {
            SwdOutcome::Failed { reason } => reason.as_str(),
            SwdOutcome::Drift { detail } => detail.as_str(),
            _ => "unknown",
        },
    );
    // Return as tool error — the runtime loop will feed this back to the model
    // The model sees this as a tool error and naturally retries
    return Err(ToolError::new(correction_hint));
}
```

**Note on partial mode vs full mode:** In partial mode, the correction is "implicit" because the runtime's agentic loop in `conversation.rs` already feeds tool errors back to the model as `ConversationMessage::tool_result(..., is_error: true)`. The model naturally sees the structured error and retries. **No explicit correction loop is needed for partial mode** — just a richer error message.

However, to prevent infinite retries at the tool level, add a per-path retry counter to `CliToolExecutor`:

```rust
// New field on CliToolExecutor:
swd_retry_counts: HashMap<String, u8>,
```

Check this counter in `execute_with_swd()`:
- If `swd_retry_counts[path] >= MAX_CORRECTION_ATTEMPTS`, return a final error without the "please retry" hint
- Otherwise increment and include the retry hint
- Clear the map when a new user turn starts (requires a `reset_correction_state()` method called from the thread spawn site)

**Acceptance criteria:**
- Partial mode: failed write tools return rich SWD error messages to the model
- The model sees the error and retries naturally via the agentic loop
- After 2 failures on the same path, the error message says "max retries exceeded" without retry hint
- Per-path counters reset on each new user turn

---

### Step 5: Reset correction state on new user turns

**File:** `crates/claw-cli/src/main.rs`
**Locations:**
- TUI thread spawn (line ~1208): Before calling `runtime.run_turn()`, reset correction state
- Non-TUI REPL loop: Same reset before each turn

For TUI mode, the `CliToolExecutor` is built fresh each turn inside `build_runtime_for_tui()`, so the `swd_retry_counts` HashMap starts empty naturally. Verify this is the case.

For `DefaultRuntimeClient`, add `correction_ctx.reset()` at the top of `stream()`.

**Acceptance criteria:**
- `CorrectionContext` resets to 0 attempts at the start of each user turn
- `swd_retry_counts` starts empty for each new turn
- A failure in turn N does not carry over correction count to turn N+1

---

### Step 6: Tests

**File:** `crates/claw-cli/src/swd.rs` (add `#[cfg(test)] mod tests`)

**6a. Unit tests for `CorrectionContext`:**
- `test_correction_context_new` — starts at 0, can_retry is true
- `test_correction_context_max_attempts` — after 2 `record_failures()` calls, `can_retry()` returns false
- `test_correction_context_reset` — after reset, attempts back to 0
- `test_correction_context_filters_only_failures` — `record_failures()` with mixed outcomes only keeps Failed/Drift/RolledBack

**6b. Unit test for `build_correction_prompt`:**
- `test_correction_prompt_format` — verify prompt contains `[SWD CORRECTION TURN]`, each failure path, status, and hashes

**6c. Integration-style test (in main.rs tests or a separate test file):**
- Mock a `ToolExecutor` that fails on first call, succeeds on second
- Verify that the rich error message contains SWD feedback
- Verify that after `MAX_CORRECTION_ATTEMPTS` failures, the message changes to "max retries exceeded"

**Acceptance criteria:**
- All unit tests pass
- `CorrectionContext` enforces the max attempts invariant
- Correction prompt contains all required fields from failed transactions

---

## Files Modified (Summary)

| File | Changes |
|------|---------|
| `crates/claw-cli/src/swd.rs` | Add `CorrectionContext`, `build_correction_prompt()`, `MAX_CORRECTION_ATTEMPTS`, unit tests |
| `crates/claw-cli/src/tui.rs` | Add `TuiMsg::CorrectionRetry`, `ChatEntry::CorrectionRetryEntry`, handle + render |
| `crates/claw-cli/src/main.rs` | Modify `DefaultRuntimeClient::stream()` for full-mode correction loop; modify `CliToolExecutor` for partial-mode rich errors + retry counters; add `swd_retry_counts` field; reset logic |

**Files NOT modified:**
- `crates/runtime/src/conversation.rs` — the generic runtime loop is not touched; correction is handled at the CLI layer
- `crates/runtime/src/*` — no runtime crate changes needed
- `crates/claw-cli/src/swd.rs` verification/rollback logic — unchanged, only new code added

---

## Preventing Infinite Loops (Safety Analysis)

1. **Const cap:** `MAX_CORRECTION_ATTEMPTS = 2` is a compile-time constant
2. **Increment before retry:** `record_failures()` increments `attempts` before the retry stream
3. **Guard before action:** `can_retry()` checked before every retry attempt
4. **Separate counters:** Full mode uses `CorrectionContext.attempts`; partial mode uses `swd_retry_counts[path]`
5. **Fresh state per turn:** Both counters reset when a new user turn begins
6. **Error escape:** Any stream/parse error in the correction path breaks the loop immediately (no retry on infrastructure errors)
7. **No recursion:** The full-mode correction loop is iterative (for loop), not recursive

---

## Success Criteria

- [ ] After SWD failure in full mode, model receives correction prompt and re-generates FILE_ACTIONs
- [ ] After SWD failure in partial mode, tool error includes structured SWD feedback that the model uses to retry
- [ ] Maximum 2 correction attempts per turn, then yields to human
- [ ] TUI shows `"↩ SWD retry 1/2"` indicator during correction turns
- [ ] Correction counter resets on each new user turn
- [ ] All new unit tests pass
- [ ] No changes to `crates/runtime/` — all correction logic lives in `crates/claw-cli/`
