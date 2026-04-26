# Open Questions

## budget-limiter - 2026-04-26
- [ ] Budget global vs per-session? — Atualmente planejado como per-session (reseta ao iniciar nova sessao). Se quiser tracking global (custo total diario/mensal), precisa de um arquivo separado de historico.
- [ ] Cache tokens contam no budget? — `UsageTracker` acumula cache_creation e cache_read separadamente. O plano assume que `total_tokens()` (que inclui cache) e o valor comparado contra `max_tokens`. Confirmar se cache tokens devem ser excluidos do budget de tokens.
- [ ] Warn_at_pct configuravel via CLI? — Atualmente so via budget.json. Adicionar `--budget-warn-pct` se necessario.
- [ ] MEMORY.md vs CLAW.md para graceful save — O projeto usa CLAW.md como memoria. Decidir se o budget save vai em CLAW.md (junto com o resto) ou em MEMORY.md separado (como o mythos-router faz).

## multi-provider-orchestrator - 2026-04-26

- [ ] Should the orchestrator support model-level routing (e.g., "use Anthropic for opus, OpenAI for gpt-4o") in addition to provider-level fallback? -- Affects whether `UnifiedProvider` needs to expose supported model lists
- [ ] Should `stream_message` support mid-stream fallback (switch provider if stream stalls), or is response-level fallback sufficient? -- Mid-stream fallback is significantly more complex and may introduce partial-response artifacts
- [ ] Should `parking_lot::RwLock` be used instead of `tokio::sync::RwLock`? -- `parking_lot` is synchronous but faster; appropriate if locks are never held across `.await`. Tokio's `RwLock` is safer for async code but has higher overhead.
- [ ] Is `async-trait` acceptable, or should we use RPITIT (return-position impl Trait in traits, stable since Rust 1.75)? -- `async-trait` adds a heap allocation per call; RPITIT avoids it but requires careful lifetime annotations for object safety
- [ ] What priority ordering should providers have by default? (Anthropic > OpenAI > xAI?) -- Affects initial scoring before enough data accumulates
- [ ] Should the `--orchestrate` flag be opt-in or opt-out once multiple API keys are detected? -- UX decision: silent orchestration vs explicit activation
- [ ] Cost tracking (`cost_per_1k` in metrics): should we ship a pricing table, or omit cost from the scoring formula initially? -- The mythos-router reference includes pricing.ts but cost data changes frequently

## skills-system - 2026-04-26
- [ ] Should skills be auto-loaded (all discovered) or require explicit activation in settings.json? — Affects whether users need to opt-in per skill or get all skills by default. TS reference requires explicit names.
- [ ] Where should `discover_skill_roots()` logic live? Currently in commands crate, but runtime needs it for prompt injection. — Architectural coupling decision: move to runtime, duplicate, or parameterize.
- [ ] Should malformed SKILL.md files produce a visible warning or be silently skipped? — User experience: silent skip hides broken skills, warnings may clutter output.
- [ ] Should `budget_multiplier` have an upper bound cap (e.g., 10.0) to prevent runaway costs? — Safety concern for when budget system is actually wired in.
- [ ] Does `/skills add <name>` need to be implemented or is file-system-based discovery sufficient? — The original request mentions it but the existing system relies on fs discovery.

## swd-correction-turns - 2026-04-26
- [ ] Should correction turns count against a token budget? The TS reference checks `budget.check()` before each retry, but claw currently has no budget system. — Could cause unexpected cost if the model generates large corrections
- [ ] Should the correction prompt include the full original assistant text (for full mode) or just the failure details? — Including full text helps the model understand context but increases token usage
- [ ] Should partial mode correction be explicit (like full mode's loop) or implicit (rely on agentic loop + rich errors)? Current plan uses implicit. — Explicit would give more control but adds complexity to the runtime layer
- [ ] Should `MAX_CORRECTION_ATTEMPTS` be user-configurable via `--swd-retries N` flag? — Current plan hardcodes to 2; a flag adds flexibility but more surface area
- [ ] For full mode corrections, should the original FILE_ACTIONs be included in the correction prompt so the model can see what it tried? — The TS reference only includes failure details, not original actions

## deterministic-response-cache - 2026-04-26

- [ ] Should the cache persist across `claw-cli` upgrades, or should a version field in the cache file invalidate old entries? — Prevents deserialization errors if `CachedResponse` schema changes.
- [ ] Should `/cache stats` also show estimated disk size of the cache file? — Useful for debugging, trivial to add.
- [ ] Is `~/.claw/cache.json` the correct path, or should it respect `XDG_CACHE_HOME` on Linux? — The codebase currently doesn't seem to use XDG conventions, but it's a common expectation.
- [ ] Should there be a max entry count cap (e.g., 500 entries) in addition to TTL? — Prevents unbounded cache growth for heavy users.
- [ ] The `ConversationClient` type is imported in `app.rs` as `runtime::ConversationClient` but is not exported from `runtime/src/lib.rs`. This may be a compile error or WIP code. The cache integration in Step 3 assumes `run_turn` is the interception point, but the exact signature needs verification at implementation time. — Affects where exactly the cache check/store is placed.

## structured-telemetry - 2026-04-26
- [ ] Should `claw stats --json` be supported for machine-readable output? — Useful for scripting/dashboards but adds scope
- [ ] Should telemetry include tool-use events (which tools were called, duration)? — Rich data for debugging but significantly increases event volume
- [ ] Is `~/.config/claw/` the correct config dir, or should it follow `dirs::data_local_dir()` (e.g. `~/.local/share/claw/` on Linux)? — Telemetry is data, not config; `data_local_dir` may be more semantically correct
- [ ] Should there be a `claw stats --clear` to purge the telemetry file? — Simple to add but could be added later
- [ ] What happens when multiple `claw` processes write to the same JSONL file concurrently? — Append-mode writes of single lines are atomic on most OSes for lines under PIPE_BUF (~4KB), but we should document this assumption

## verify-command - 2026-04-26
- [ ] Deteccao de drift: o TypeScript usa `entry.result.includes('OK')` para detectar drift, mas os instruction files do claw (CLAW.md) nao tem format de action/result como o MEMORY.md do mythos. Precisa decidir: drift e apenas "memoria diz DELETE mas arquivo existe"? Ou incluir deteccao por hash/timestamp? — Impacta complexidade do Step 1
- [ ] Profundidade do parse de paths: backtick paths (`` `src/foo.rs` ``), paths em code blocks, paths em prosa natural -- ate que ponto o parser deve ir? — Impacta falsos positivos no report
- [ ] Suporte a .gitignore em subdiretorios: o gitignore spec permite .gitignore em qualquer diretorio. O plan atual so le da raiz. Implementar hierarquia completa adiciona complexidade. — Pode ser deferido para v2
- [ ] Exit code semantico: `claw verify` deve retornar exit code 1 quando ha drift/missing? Isso afeta uso em CI/CD. — Decisao de UX
