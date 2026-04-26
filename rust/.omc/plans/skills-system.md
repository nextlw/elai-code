# Plan: Skills System via SKILL.md

**Date:** 2026-04-26
**Complexity:** MEDIUM
**Scope:** 4 files modified, 1 new file, ~600 lines

---

## Context

The elai CLI already has a partial skills infrastructure in `crates/commands/src/lib.rs`:
- `discover_skill_roots()` finds SKILL.md files across `.elai/skills/`, `.codex/skills/`, `~/.elai/skills/`, etc.
- `parse_skill_frontmatter()` extracts only `name` and `description` (2 fields)
- `/skills list` slash command works but is display-only
- **Skills are never loaded into the system prompt** -- this is the core gap

The TypeScript reference (`mythos-router/src/skills.ts`) shows the full protocol: YAML frontmatter with priority, budget_multiplier, incompatible_with, force_provider, plus injection into the system prompt sorted by priority.

### Architectural Decision

**Parser approach: Manual YAML parsing (no serde_yaml)**

- The project already uses manual frontmatter parsing in `parse_skill_frontmatter()`
- The TS reference uses zero-dep manual parsing
- SKILL.md frontmatter is flat key-value + simple arrays -- no nested YAML needed
- Adding `serde_yaml` would introduce a heavy transitive dependency tree for a simple use case
- Consistent with project philosophy (minimal deps: see Cargo.toml -- only serde_json, regex, glob)

---

## Work Objectives

1. Create a full `Skill` model with metadata in `crates/runtime/src/skills.rs`
2. Inject loaded skills into the system prompt via `SystemPromptBuilder`
3. Enhance `/skills` slash command with richer metadata display
4. Validate skill compatibility (incompatibilities, provider conflicts)
5. Surface `budget_multiplier` for downstream consumption

---

## Guardrails

### Must Have
- Zero new external crate dependencies (manual frontmatter parsing)
- Backward-compatible with existing SKILL.md files that only have `name`/`description`
- Skills sorted by priority (descending) before injection
- Incompatibility detection with clear error messages
- Unit tests for parsing, loading, sorting, and validation

### Must NOT Have
- serde_yaml or any YAML crate dependency
- Breaking changes to existing `parse_skill_frontmatter()` in commands crate
- Changes to the API client or message protocol
- Automatic skill activation (skills must be explicitly configured)

---

## Task Flow

### Step 1: Create `crates/runtime/src/skills.rs` -- Skill Model + Parser

**New file:** `crates/runtime/src/skills.rs`

```
// Structs
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub version: String,
    pub priority: i32,                        // default: 50, higher = loaded first
    pub budget_multiplier: f32,               // default: 1.0
    pub force_provider: Option<String>,       // e.g. "opus", "sonnet"
    pub incompatible_with: Vec<String>,       // skill names this conflicts with
    pub requires_tools: Vec<String>,          // tool names needed
    pub allow_fallback: bool,                 // default: true
    pub max_output_tokens: Option<u32>,
    pub timeout_ms: Option<u64>,
}

pub struct Skill {
    pub metadata: SkillMetadata,
    pub body: String,           // markdown instructions after frontmatter
    pub file_path: PathBuf,
}

pub struct SkillValidation {
    pub valid: bool,
    pub errors: Vec<String>,
}

pub struct SkillPromptResult {
    pub sections: Vec<String>,              // prompt sections to append
    pub budget_multiplier: f32,             // aggregated multiplier
    pub force_provider: Option<String>,     // first provider directive found
    pub max_output_tokens: Option<u32>,     // minimum across skills
    pub timeout_ms: Option<u64>,            // minimum across skills
}
```

**Functions to implement:**

1. `fn parse_frontmatter(content: &str) -> (HashMap<String, FrontmatterValue>, String)`
   - Regex: `^---\r?\n([\s\S]*?)\r?\n---\r?\n([\s\S]*)$`
   - Handle: strings, numbers (i32/f32), booleans, simple arrays (`- item`)
   - `FrontmatterValue` enum: `Str(String)`, `Int(i32)`, `Float(f32)`, `Bool(bool)`, `List(Vec<String>)`

2. `fn load_skill(path: &Path) -> Result<Skill, SkillLoadError>`
   - Read file, parse frontmatter, construct Skill with defaults for missing fields

3. `fn load_skills_from_dir(dir: &Path) -> Result<Vec<Skill>, SkillLoadError>`
   - Iterate subdirectories, look for `SKILL.md` in each
   - Sort by priority descending
   - Skip malformed skills with warning (log, don't fail)

4. `fn load_all_skills(cwd: &Path) -> Vec<Skill>`
   - Reuse the same root discovery logic as `discover_skill_roots()` in commands crate
   - Or: accept a `Vec<PathBuf>` of root dirs from outside
   - Deduplicate by name (first occurrence wins, matching existing shadowing logic)

5. `fn validate_skills(skills: &[Skill]) -> SkillValidation`
   - Check incompatible_with conflicts (bidirectional)
   - Check force_provider conflicts (multiple skills forcing different providers)

6. `fn build_skill_prompt_sections(skills: &[Skill]) -> SkillPromptResult`
   - Sort by priority descending
   - Format each skill as:
     ```
     ## ACTIVE SKILL: {name} (v{version})
     Priority: {priority} | Budget: {budget_multiplier}x
     
     {body}
     ```
   - Wrap all in a header: `# Active Skills\nThe following {n} skill(s) are loaded.`
   - Aggregate: budget_multiplier (product), max_output_tokens (min), timeout_ms (min), first force_provider

**Acceptance Criteria:**
- [ ] `parse_frontmatter` handles all value types including arrays
- [ ] Missing fields use sensible defaults (priority=50, budget_multiplier=1.0, allow_fallback=true)
- [ ] `validate_skills` catches both incompatibility and provider conflicts
- [ ] Unit tests: parse valid frontmatter, parse missing fields, parse arrays, invalid frontmatter returns empty meta

### Step 2: Wire Skills into System Prompt

**Files:** `crates/runtime/src/prompt.rs`, `crates/runtime/src/lib.rs`

Changes:

1. In `prompt.rs`, add to `SystemPromptBuilder`:
   ```rust
   skills: Vec<Skill>,  // new field
   
   pub fn with_skills(mut self, skills: Vec<Skill>) -> Self {
       self.skills = skills;
       self
   }
   ```

2. In `build()` method, after `self.append_sections` but before the final join, inject skill sections:
   ```rust
   if !self.skills.is_empty() {
       let result = build_skill_prompt_sections(&self.skills);
       sections.extend(result.sections);
   }
   ```

3. In `load_system_prompt()` function, load skills and pass to builder:
   ```rust
   let skills = load_all_skills(&cwd);
   // ... existing builder chain ...
   .with_skills(skills)
   ```

4. In `lib.rs`, add `mod skills;` and export public types.

**Acceptance Criteria:**
- [ ] Skills appear in system prompt output (verifiable via `elai --print-system-prompt`)
- [ ] Skills are ordered by priority (highest first)
- [ ] Empty skills dir produces no skill sections
- [ ] Existing tests in `prompt.rs` continue to pass

### Step 3: Enhance `/skills` Slash Command

**File:** `crates/commands/src/lib.rs`

The existing `/skills list` only shows name + description. Enhance to show:
- Priority
- Budget multiplier (if != 1.0)
- Provider force (if set)
- Incompatibilities (if any)
- Active vs shadowed status (already exists)

**Approach:** Either:
- (A) Import and use the new `runtime::skills` module types in the commands crate rendering, or
- (B) Extend the existing `SkillSummary` struct with optional metadata fields and enrich `parse_skill_frontmatter()` to extract more fields

Option (B) is preferred to avoid coupling the commands crate to the full `Skill` struct and to keep the existing `discover_skill_roots` flow intact.

Add optional fields to `SkillSummary`:
```rust
struct SkillSummary {
    name: String,
    description: Option<String>,
    source: DefinitionSource,
    shadowed_by: Option<DefinitionSource>,
    origin: SkillOrigin,
    // New fields:
    priority: Option<i32>,
    budget_multiplier: Option<f32>,
    force_provider: Option<String>,
    incompatible_with: Vec<String>,
}
```

Update `render_skills_report()` to display the extra metadata.

**Acceptance Criteria:**
- [ ] `/skills list` shows priority and budget multiplier for each skill
- [ ] Existing test `render_skills_report` updated to match new format
- [ ] Skills without extended metadata still display correctly

### Step 4: Expose Budget Multiplier + Provider to CLI

**Files:** `crates/runtime/src/prompt.rs`, `crates/elai-cli/src/main.rs`

The `build_skill_prompt_sections()` returns a `SkillPromptResult` with `budget_multiplier` and `force_provider`. These need to be surfaced to the caller so the CLI can act on them.

1. Change `load_system_prompt()` return type or create a new `SystemPromptOutput` struct:
   ```rust
   pub struct SystemPromptOutput {
       pub sections: Vec<String>,
       pub skill_budget_multiplier: f32,
       pub skill_force_provider: Option<String>,
   }
   ```

2. In `main.rs` `build_system_prompt()`, use the new return type and pass `budget_multiplier` to the runtime configuration (or store for later use).

3. If the project adds explicit token budgeting later, the multiplier is ready. For now, surface it as metadata on the system prompt output.

**Acceptance Criteria:**
- [ ] `SystemPromptOutput` carries budget_multiplier from skills
- [ ] CLI can read and log the active budget multiplier
- [ ] No behavioral change when no skills are loaded (multiplier defaults to 1.0)

### Step 5: Tests

**File:** `crates/runtime/src/skills.rs` (inline `#[cfg(test)]` module)

Test cases:

1. **Frontmatter parsing:**
   - Valid full frontmatter with all fields
   - Minimal frontmatter (only name)
   - Missing frontmatter (no `---` delimiters) -- returns defaults
   - Array fields (incompatible_with, requires_tools)
   - Quoted vs unquoted string values
   - Boolean and numeric parsing

2. **Skill loading:**
   - Load from directory with multiple skills
   - Skip directories without SKILL.md
   - Handle empty directory gracefully

3. **Priority sorting:**
   - Skills with different priorities sorted descending
   - Skills with same priority maintain stable order

4. **Validation:**
   - Two skills with mutual incompatibility detected
   - Two skills forcing different providers detected
   - Valid set passes validation

5. **Prompt generation:**
   - Skills rendered in correct format
   - Budget multiplier aggregated correctly (product of all multipliers)
   - Provider taken from first skill with force_provider

**Acceptance Criteria:**
- [ ] All tests pass with `cargo test -p runtime`
- [ ] Edge cases covered (empty dir, malformed SKILL.md, missing fields)

---

## File Change Summary

| File | Action | Description |
|------|--------|-------------|
| `crates/runtime/src/skills.rs` | **CREATE** | Skill model, parser, loader, validator, prompt builder |
| `crates/runtime/src/lib.rs` | MODIFY | Add `mod skills;` and public exports |
| `crates/runtime/src/prompt.rs` | MODIFY | Add `with_skills()` to builder, inject sections in `build()`, change `load_system_prompt()` return |
| `crates/commands/src/lib.rs` | MODIFY | Enrich `SkillSummary` with metadata, update rendering |
| `crates/elai-cli/src/main.rs` | MODIFY | Adapt to new `SystemPromptOutput` return type |

## Dependencies

**No new crate dependencies.** Manual frontmatter parsing using:
- `str::lines()`, `str::strip_prefix()`, `str::parse::<f32>()` etc.
- Regex from already-imported `regex` crate (for frontmatter delimiter matching)

## Open Design Decisions

1. **Skill activation model:** Currently the plan loads ALL discovered skills into the prompt. Should there be an explicit activation mechanism (e.g., `active_skills` list in `.elai/settings.json`)? The TS reference requires explicit `skillNames` to be passed. Recommend: load all discovered skills by default (matching how instruction files work), but allow disabling via settings.
2. **Shared root discovery:** The `discover_skill_roots()` logic lives in the `commands` crate. The runtime crate needs similar logic. Options: (A) move it to runtime and re-export, (B) duplicate with slight variation, (C) accept root dirs as parameter. Recommend (C) -- have `load_system_prompt` accept skill dirs, let the CLI resolve them.
3. **Budget multiplier consumption:** No explicit budget system exists yet. The multiplier is surfaced but not consumed. This is intentional scaffolding for future budget features.
