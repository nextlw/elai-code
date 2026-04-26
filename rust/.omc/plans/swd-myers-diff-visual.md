# SWD Myers Diff Visual

**Date:** 2026-04-26
**Status:** Draft
**Complexity:** MEDIUM

---

## Context

The claw CLI (Rust) already implements SWD (Strict Write Discipline) in `crates/claw-cli/src/swd.rs` with transactional file writes, snapshots, rollback, and `[FILE_ACTION]` parsing. In full mode, when the assistant finishes a response, `parse_file_actions` extracts actions and `execute_file_actions` applies them atomically. Results are displayed in the TUI as a compact log (`SwdLogEntry`).

The TypeScript reference in `mythos-router/src/diff.ts` implements a Myers diff algorithm with backtracking that produces `DiffLine { op, val }` entries and renders them with ANSI colors and line numbers. The `swd-cli.ts` file shows the integration: before applying each `FileAction`, the old content is snapshotted, a diff is computed against the new content, rendered, and the user is prompted to accept or reject each action individually.

**Goal:** Port this diff-preview flow into the Rust TUI so that in SWD Full mode, the user sees a colored diff for each file before actions are applied, and can accept or reject the batch.

---

## Work Objectives

1. Add a `diff.rs` module with Myers diff computation and context-window generation
2. Add a `SwdDiffPreview` chat entry type with ratatui rendering (green/red/gray, line numbers, 3-line context)
3. Integrate diff preview into the SWD full-mode flow: snapshot before -> compute diff -> show preview -> wait for user confirmation -> execute or discard
4. Add a `ToolApproval`-style overlay for accept/reject of pending file actions

---

## Guardrails

### Must Have
- Myers diff produces correct output for add/remove/keep operations
- Line numbers rendered on the left margin for old-file and new-file lines
- Context window of 3 lines around each change (collapsible keep regions replaced with `@@ ... @@` separator)
- User can accept or reject the entire batch before execution
- Streaming is NOT blocked while diff is computed (diff happens after `MessageStop`, before `execute_file_actions`)

### Must NOT Have
- No external diff binary dependency (no shelling out to `diff` or `git diff`)
- No changes to the SWD partial-mode flow (partial mode uses tool-call interceptors, not FILE_ACTION blocks)
- No file-by-file interactive approval in v1 (accept/reject is for the whole batch)

---

## Decision: `similar` crate vs hand-rolled Myers

**Use the `similar` crate (v2.x).**

Rationale:
- `similar` is the de-facto Rust diff library (10M+ downloads, used by `insta`, `cargo-nextest`, etc.)
- Implements Myers diff + patience diff + LCS, with `TextDiff` high-level API that handles line splitting, context hunks, and unified diff formatting out of the box
- `similar::TextDiff::from_lines(old, new)` returns an iterator of `DiffOp` / `Change` with `ChangeTag::{Equal, Insert, Delete}` -- maps directly to the needed `Keep/Add/Remove`
- Hand-rolling Myers only makes sense if we need custom backtracking behavior; we don't
- The `similar` crate is `no_std`-compatible and has zero transitive dependencies

Alternative considered (hand-rolled from `diff.ts`): rejected because it would replicate ~80 lines of well-tested algorithm for no benefit, and we'd lose context-hunk generation for free.

---

## Task Flow

### Step 1: Add `similar` dependency and create `diff.rs` module

**Files:**
- `crates/claw-cli/Cargo.toml` -- add `similar = "2"`
- `crates/claw-cli/src/diff.rs` -- new file
- `crates/claw-cli/src/main.rs` -- add `mod diff;`

**Implementation:**

```
// diff.rs public API surface:

pub enum DiffTag { Keep, Add, Remove }

pub struct DiffLine {
    pub tag: DiffTag,
    pub old_lineno: Option<usize>,  // line number in old file (1-based)
    pub new_lineno: Option<usize>,  // line number in new file (1-based)
    pub value: String,
}

pub struct DiffHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

/// Computes a unified diff with `context` lines around each change.
/// Returns a list of hunks (like git diff @@ sections).
pub fn compute_diff(old: &str, new: &str, context: usize) -> Vec<DiffHunk>

/// Returns true if old == new (no changes).
pub fn is_unchanged(old: &str, new: &str) -> bool
```

Internally uses `similar::TextDiff::from_lines` and groups changes into hunks with the requested context window (default 3).

**Acceptance criteria:**
- `compute_diff("a\nb\nc", "a\nX\nc", 3)` returns 1 hunk with Keep("a"), Remove("b"), Add("X"), Keep("c") and correct line numbers
- `compute_diff` on identical strings returns empty vec
- `compute_diff` on completely new file (old = "") returns 1 hunk with all Add lines
- `compute_diff` on file deletion (new = "") returns 1 hunk with all Remove lines

---

### Step 2: Add `SwdDiffPreview` to TUI chat model and message channel

**Files:**
- `crates/claw-cli/src/tui.rs` -- modify `TuiMsg`, `ChatEntry`, `OverlayKind`

**Changes:**

1. Add to `TuiMsg`:
```rust
SwdDiffPreview {
    actions: Vec<(String, Vec<crate::diff::DiffHunk>)>,  // (path, hunks)
    reply_tx: mpsc::SyncSender<bool>,  // true = accept, false = reject
}
```

2. Add to `ChatEntry`:
```rust
SwdDiffEntry {
    path: String,
    hunks: Vec<crate::diff::DiffHunk>,
}
```

3. Add to `OverlayKind`:
```rust
SwdConfirmApply {
    action_count: usize,
    reply_tx: mpsc::SyncSender<bool>,
}
```

4. In `UiApp::handle_msg`: when `SwdDiffPreview` arrives, push one `SwdDiffEntry` per file into `chat`, then open `SwdConfirmApply` overlay.

**Acceptance criteria:**
- `TuiMsg::SwdDiffPreview` can be constructed and sent through the `mpsc::SyncSender<TuiMsg>` channel
- Receiving `SwdDiffPreview` pushes N `SwdDiffEntry` items into `app.chat` and opens the overlay
- Overlay shows `[A] Accept  [R] Reject` and sends `true`/`false` back through `reply_tx`

---

### Step 3: Render diff hunks in `chat_to_lines`

**Files:**
- `crates/claw-cli/src/tui.rs` -- add `SwdDiffEntry` arm in `chat_to_lines`

**Rendering rules** (matching `diff.ts` reference style):

```
  --- path/to/file.rs
  @@ -10,7 +10,8 @@
   10 |   unchanged line        (Color::DarkGray)
   11 | - removed line          (Color::Red, BOLD)
      | + added line            (Color::Green, BOLD)
   12 |   unchanged line        (Color::DarkGray)
```

- Header line: `--- {path}` in Cyan+Bold
- Hunk separator: `@@ -{old_start},{old_count} +{new_start},{new_count} @@` in Magenta
- Keep lines: `{old_lineno:>4} |   {value}` in DarkGray
- Remove lines: `{old_lineno:>4} | - {value}` in Red+Bold
- Add lines: `     | + {value}` in Green+Bold
- Line number column is 4 chars wide, right-aligned

**Acceptance criteria:**
- A `SwdDiffEntry` with 1 hunk of [Keep, Remove, Add, Keep] renders exactly 6 lines (header + hunk separator + 4 content lines)
- Colors match the specification above
- Long lines are truncated to terminal width (no wrapping in diff view)
- Empty hunks vector renders `(No changes detected)` in DarkGray

---

### Step 4: Integrate diff preview into SWD full-mode execution flow

**Files:**
- `crates/claw-cli/src/main.rs` -- modify the SWD full-mode block (~line 3732-3749)

**Current flow:**
```
MessageStop -> parse_file_actions -> execute_file_actions -> SwdBatchResult
```

**New flow:**
```
MessageStop -> parse_file_actions -> for each action: snapshot old content, compute_diff(old, new)
           -> send SwdDiffPreview { actions_with_diffs, reply_tx }
           -> block on reply_rx.recv()
           -> if accepted: execute_file_actions -> SwdBatchResult
           -> if rejected: send SystemNote("SWD: batch rejected by user") -> clear full_text_buf
```

**Key implementation details:**
- The `reply_tx`/`reply_rx` pair uses `mpsc::sync_channel(1)` (same pattern as `PermRequest`)
- The background thread blocks on `reply_rx.recv()` while the TUI renders the diff and overlay
- If `tui_sender` is None (non-TUI mode / rustyline), skip diff preview and execute directly (preserve existing behavior)
- For new files (snapshot returns None), diff is computed against empty string
- For delete operations, diff is computed against old content with new = empty string

**Acceptance criteria:**
- In TUI + SWD Full mode: file actions are NOT applied until user presses Accept
- Pressing Reject discards all actions and shows a system note
- In non-TUI mode: behavior unchanged (actions execute immediately as before)
- The background thread does not deadlock (reply channel has capacity 1)

---

### Step 5: Handle keyboard input for SwdConfirmApply overlay

**Files:**
- `crates/claw-cli/src/tui.rs` -- add key handling in the overlay match block

**Overlay rendering:**
```
┌─── SWD: Apply N file change(s)? ───┐
│                                      │
│   [A] Accept    [R] Reject           │
│                                      │
└──────────────────────────────────────┘
```

**Key bindings:**
- `a` or `Enter` -> send `true` through `reply_tx`, close overlay
- `r` or `Escape` -> send `false` through `reply_tx`, close overlay

**Acceptance criteria:**
- Overlay renders centered on screen with the action count
- `a`/`Enter` sends `true` and closes overlay
- `r`/`Escape` sends `false` and closes overlay
- After overlay closes, the diff entries remain visible in chat history (for reference)

---

## Dependencies to Add

| Crate | Version | Purpose |
|-------|---------|---------|
| `similar` | `"2"` | Myers/patience diff algorithm with unified diff hunk generation |

No other new dependencies required. `ratatui`, `crossterm`, `sha2`, `hex` are already present.

---

## Files Modified (Summary)

| File | Change |
|------|--------|
| `crates/claw-cli/Cargo.toml` | Add `similar = "2"` |
| `crates/claw-cli/src/diff.rs` | **NEW** -- diff computation module |
| `crates/claw-cli/src/main.rs` | Add `mod diff;` + modify SWD full-mode block |
| `crates/claw-cli/src/tui.rs` | Add `SwdDiffPreview` msg, `SwdDiffEntry` chat entry, `SwdConfirmApply` overlay, diff rendering in `chat_to_lines`, key handling |
| `crates/claw-cli/src/swd.rs` | No changes needed (snapshot already exposes raw bytes) |

---

## Success Criteria

1. In SWD Full mode, when the assistant emits `[FILE_ACTION]` blocks, the user sees a colored unified diff in the chat panel before any file is written
2. The diff shows line numbers, +/- markers, and 3-line context around changes, matching standard unified diff format
3. The user must explicitly accept (press A/Enter) before actions are applied
4. Rejecting discards all pending actions and shows a notification
5. Non-TUI mode continues to work without diff preview (no regression)
6. The `similar` crate handles all diff computation; no hand-rolled algorithm
