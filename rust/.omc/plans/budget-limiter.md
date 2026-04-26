# Budget Limiter com Tracking de Custo USD em Tempo Real

**Data:** 2026-04-26
**Complexidade:** MEDIUM
**Escopo:** ~6 arquivos modificados, 1 arquivo novo

---

## Context

O projeto claw-code/rust ja possui infraestrutura solida de tracking de uso em `crates/runtime/src/usage.rs`:
- `ModelPricing` — tabela de precos por modelo (Sonnet, Haiku, Opus, GPT-4o, GPT-4.1, etc.)
- `TokenUsage` — struct com input/output/cache_creation/cache_read tokens
- `UsageCostEstimate` — calculo de custo USD com `total_cost_usd()`
- `UsageTracker` — acumulador com `record()`, `cumulative_usage()`, `turns()`
- `pricing_for_model()` — lookup de pricing por nome de modelo
- `format_usd()` — formatacao `$X.XXXX`

O que **falta**:
1. Conceito de "budget" (limites configurados pelo usuario)
2. Verificacao de limites a cada turno
3. Status de warning/exhausted
4. Persistencia de budget entre sessoes
5. Integracao com TUI (barra de progresso, mensagens de warning)
6. Slash command e CLI flags

**Referencia TypeScript:** `mythos-router/src/budget.ts` — `SessionBudget` com `check()` retornando `BudgetCheck { ok, exhausted, warning, reason }` e `formatBar()`.

---

## Work Objectives

Implementar um budget limiter que:
- Permite ao usuario definir limites de tokens, turnos e custo USD
- Verifica limites apos cada turno da API
- Exibe barra de progresso no footer da TUI
- Emite warnings ao atingir threshold configuravel
- Salva contexto gracefully ao atingir limite (MEMORY.md)
- Persiste configuracao de budget entre sessoes

---

## Guardrails

### Must Have
- Budget tracking reutiliza `UsageTracker` e `pricing_for_model()` existentes
- Budget desativado por default (opt-in via CLI flags ou `/budget`)
- Graceful shutdown: ao atingir limite, salva resumo em MEMORY.md e encerra o turno
- Todos os campos opcionais: usuario pode definir so tokens, so USD, ou ambos

### Must NOT Have
- Budget nao bloqueia startup (se budget.json corrompido, ignora e continua)
- Nao modifica a struct `UsageTracker` existente (composicao, nao heranca)
- Nao adiciona dependencias externas novas

---

## Task Flow

### Step 1: `BudgetConfig` + `BudgetTracker` + `BudgetStatus` no runtime crate

**Arquivo:** `crates/runtime/src/budget.rs` (NOVO)

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_tokens: Option<u64>,
    pub max_turns: Option<u32>,
    pub max_cost_usd: Option<f64>,
    pub warn_at_pct: f32,  // default 80.0
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_tokens: None,
            max_turns: None,
            max_cost_usd: None,
            warn_at_pct: 80.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BudgetStatus {
    Ok,
    Warning { pct: f32, dimension: &'static str },
    Exhausted { reason: String },
    Disabled,
}

pub struct BudgetTracker {
    config: BudgetConfig,
    enabled: bool,
}

impl BudgetTracker {
    pub fn new(config: BudgetConfig) -> Self { ... }
    pub fn disabled() -> Self { ... }

    /// Verifica budget contra o estado atual do UsageTracker.
    /// Recebe UsageTracker + model name para calcular custo corrente.
    pub fn check(&self, usage: &UsageTracker, model: &str) -> BudgetStatus { ... }

    /// Retorna porcentagem de uso por dimensao (tokens, turns, cost).
    pub fn usage_pct(&self, usage: &UsageTracker, model: &str) -> BudgetUsagePct { ... }

    pub fn is_enabled(&self) -> bool { ... }
    pub fn config(&self) -> &BudgetConfig { ... }

    /// Atualiza config em runtime (para /budget slash command).
    pub fn update_config(&mut self, config: BudgetConfig) { ... }
}

#[derive(Debug, Clone, Copy)]
pub struct BudgetUsagePct {
    pub tokens_pct: f32,
    pub turns_pct: f32,
    pub cost_pct: f32,
    pub highest_pct: f32,       // max dos tres
    pub current_cost_usd: f64,  // custo acumulado
    pub total_tokens: u64,
}
```

**Logica de `check()`:**
1. Se `!enabled` -> `BudgetStatus::Disabled`
2. Calcula `cumulative.total_tokens()` vs `max_tokens`
3. Calcula `usage.turns()` vs `max_turns`
4. Calcula custo USD via `cumulative.estimate_cost_usd_with_pricing(pricing_for_model(model))` vs `max_cost_usd`
5. Se qualquer dimensao >= 100% -> `Exhausted { reason }`
6. Se qualquer dimensao >= `warn_at_pct` -> `Warning { pct, dimension }`
7. Senao -> `Ok`

**Persistencia** (`budget.json`):
```rust
/// Carrega de ~/.config/claw/budget.json ou .claw/budget.json
pub fn load_budget_config(cwd: &Path) -> Option<BudgetConfig> { ... }

/// Salva para .claw/budget.json (projeto local)
pub fn save_budget_config(cwd: &Path, config: &BudgetConfig) -> io::Result<()> { ... }
```

**Acceptance Criteria:**
- `BudgetTracker::check()` retorna `Exhausted` quando qualquer limite e ultrapassado
- `BudgetTracker::check()` retorna `Warning` quando qualquer dimensao >= warn_at_pct
- `BudgetTracker::disabled()` sempre retorna `Disabled`
- Unit tests para cada cenario (Ok, Warning tokens, Warning cost, Exhausted turns, Exhausted cost)
- `budget.json` round-trip: serialize -> deserialize produz mesmo config

**Arquivo modificado:** `crates/runtime/src/lib.rs` — adicionar `pub mod budget;` e re-export dos tipos publicos.

---

### Step 2: CLI Flags `--budget-tokens`, `--budget-usd`, `--budget-turns`, `--no-budget`

**Arquivo:** `crates/claw-cli/src/main.rs`

Adicionar ao `parse_args()` (seguindo o padrao de `--swd`):

```rust
"--budget-tokens" => {
    let value = args.get(index + 1)
        .ok_or_else(|| "missing value for --budget-tokens".to_string())?;
    budget_max_tokens = Some(value.parse::<u64>()
        .map_err(|_| format!("invalid --budget-tokens: {value}"))?);
    index += 2;
}
"--budget-usd" => {
    let value = args.get(index + 1)
        .ok_or_else(|| "missing value for --budget-usd".to_string())?;
    budget_max_usd = Some(value.parse::<f64>()
        .map_err(|_| format!("invalid --budget-usd: {value}"))?);
    index += 2;
}
"--budget-turns" => { /* mesmo padrao */ }
"--no-budget" => { no_budget = true; index += 1; }
```

Adicionar campos ao `CliAction::Repl`:
```rust
Repl {
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    no_tui: bool,
    swd_level: crate::swd::SwdLevel,
    budget_config: Option<BudgetConfig>,  // NOVO
}
```

Propagar `budget_config` para `run_repl()` e `run_tui_repl()`.

**Acceptance Criteria:**
- `--budget-tokens 500000` resulta em `BudgetConfig { max_tokens: Some(500000), .. }`
- `--budget-usd 5.0` resulta em `BudgetConfig { max_cost_usd: Some(5.0), .. }`
- `--no-budget` desativa budget mesmo se budget.json existir
- Sem flags de budget + sem budget.json = budget desativado (default)
- Help text (`print_help`) documenta as novas flags

---

### Step 3: Integracao com TUI — footer com barra de progresso

**Arquivos:** `crates/claw-cli/src/tui.rs`, `crates/claw-cli/src/main.rs`

3a. Adicionar campos ao `UiApp`:
```rust
pub budget_tracker: Arc<Mutex<BudgetTracker>>,
```

3b. Adicionar variantes ao `TuiMsg`:
```rust
TuiMsg::BudgetWarning(f32),       // pct de consumo
TuiMsg::BudgetExhausted(String),  // reason
```

3c. Modificar `draw_status()` para incluir barra de progresso quando budget ativo:

Formato do footer com budget ativo:
```
 · Model sonnet · Perm workspace-write · [|||||   ] 67% · $2.43 · SWD:off · abc123
```

Logica da barra (8 chars):
```rust
fn budget_bar(pct: f32) -> String {
    let filled = ((pct / 100.0) * 8.0).round() as usize;
    let empty = 8 - filled.min(8);
    let color = if pct >= 90.0 { "red" } else if pct >= 80.0 { "yellow" } else { "green" };
    format!("[{}{}]", "|".repeat(filled), " ".repeat(empty))
}
```

Cores do ratatui:
- pct < 80: `Color::Green`
- 80 <= pct < 90: `Color::Yellow`
- pct >= 90: `Color::Red`

3d. Processar `BudgetWarning` e `BudgetExhausted` em `apply_tui_msg()`:
- Warning: `push_chat(SystemNote("Warning: Budget 85% consumed — $4.25 of $5.00"))
- Exhausted: `push_chat(SystemNote("Budget exhausted: ..."))`, set `thinking = false`

3e. No `run_tui_repl()`, apos cada turno completar (no handler de `thread_done_rx`):
```rust
let budget_status = budget_tracker.lock().unwrap().check(&usage_from_session, &model);
match budget_status {
    BudgetStatus::Exhausted { reason } => {
        save_memory_summary(...);  // graceful save
        app.push_chat(SystemNote(format!("Budget exhausted: {reason}")));
    }
    BudgetStatus::Warning { pct, .. } => {
        app.push_chat(SystemNote(format!("Warning: Budget {pct:.0}% consumed")));
    }
    _ => {}
}
```

**Acceptance Criteria:**
- Footer mostra barra + custo quando budget ativo
- Footer normal (sem barra) quando budget desativado
- Warning aparece como SystemNote amarelo no chat
- Exhausted para o thinking e mostra mensagem no chat
- Cores da barra mudam conforme porcentagem

---

### Step 4: `/budget` Slash Command

**Arquivos:** `crates/commands/src/lib.rs`, `crates/claw-cli/src/main.rs`

4a. Adicionar ao enum `SlashCommand`:
```rust
Budget {
    max_tokens: Option<String>,
    max_usd: Option<String>,
},
```

4b. Adicionar parse em `SlashCommand::parse()`:
```rust
"budget" => {
    let max_tokens = parts.next().map(ToOwned::to_owned);
    let max_usd = parts.next().map(ToOwned::to_owned);
    Self::Budget { max_tokens, max_usd }
}
```

Uso:
- `/budget` — mostra status atual do budget
- `/budget 500000` — define max_tokens
- `/budget 500000 5.0` — define max_tokens e max_usd
- `/budget off` — desativa budget

4c. Adicionar ao `slash_palette_items()` em tui.rs:
```rust
("budget".into(), "Budget limiter (tokens/custo)".into()),
```

4d. Adicionar ao `handle_tui_slash_command()`:
```rust
"budget" => {
    // parse args, update budget_tracker, show status
}
```

4e. Adicionar ao `LiveCli::handle_repl_slash_command()` para REPL plain-text.

**Acceptance Criteria:**
- `/budget` sem args mostra snapshot: tokens usados, custo, limites ativos
- `/budget 500000` atualiza max_tokens em runtime
- `/budget off` desativa budget
- Comando aparece na slash palette (Ctrl+K)

---

### Step 5: Graceful Save (MEMORY.md) ao Atingir Limite

**Arquivo:** `crates/claw-cli/src/main.rs`

Quando `BudgetStatus::Exhausted`:
1. Gerar resumo:
   ```
   ## Budget Save — {timestamp}
   - Tokens: {used}/{max} ({pct}%)
   - Turns: {turns}
   - Cost: ${cost:.4}
   - Model: {model}
   - Session: {session_id}
   - Last topic: {last_user_message_preview}
   ```
2. Append ao `MEMORY.md` no diretorio do projeto (ou `CLAW.md` se existir)
3. Salvar sessao normalmente (ja acontece em `thread_done_rx`)
4. Mostrar mensagem no chat com instrucoes de como aumentar limite

**Acceptance Criteria:**
- MEMORY.md recebe bloco com timestamp, metricas e preview da ultima mensagem
- Sessao e salva normalmente (pode ser retomada com `/session`)
- Mensagem no chat indica como aumentar: `--budget-tokens N` ou `/budget N`

---

### Step 6: Persistencia de Budget Config em `budget.json`

**Arquivo:** `crates/runtime/src/budget.rs` (funcoes de I/O), `crates/claw-cli/src/main.rs` (carregamento)

Fluxo:
1. No startup (`run_tui_repl` / `run_repl`):
   - Se CLI flags presentes -> usa como config
   - Senao, tenta carregar `.claw/budget.json`
   - Senao, budget desativado
2. `/budget 500000 5.0` -> salva em `.claw/budget.json`
3. `/budget off` -> remove `.claw/budget.json`

Formato `budget.json`:
```json
{
  "max_tokens": 500000,
  "max_turns": null,
  "max_cost_usd": 5.0,
  "warn_at_pct": 80.0
}
```

**Acceptance Criteria:**
- Budget config sobrevive restart: definir via `/budget`, sair, abrir de novo
- CLI flags tem prioridade sobre budget.json
- `--no-budget` ignora budget.json
- budget.json corrompido nao impede startup (log warning, continua sem budget)

---

## Files Summary

| Acao | Arquivo | Descricao |
|------|---------|-----------|
| CRIAR | `crates/runtime/src/budget.rs` | BudgetConfig, BudgetTracker, BudgetStatus, BudgetUsagePct, persistencia |
| MODIFICAR | `crates/runtime/src/lib.rs` | `pub mod budget;` + re-exports |
| MODIFICAR | `crates/commands/src/lib.rs` | `SlashCommand::Budget` variant + parse |
| MODIFICAR | `crates/claw-cli/src/main.rs` | CLI flags, CliAction::Repl fields, run_tui_repl integracao, handle_tui_slash_command, graceful save |
| MODIFICAR | `crates/claw-cli/src/tui.rs` | UiApp.budget_tracker, TuiMsg variants, draw_status barra, apply_tui_msg handlers, slash palette |

---

## Success Criteria

1. `cargo test -p runtime` passa — inclui novos testes de BudgetTracker
2. `cargo test -p claw-cli` passa — inclui testes de parse de CLI flags
3. `claw --budget-usd 0.50` inicia com budget ativo, footer mostra barra
4. Ao atingir $0.50, sessao encerra gracefully com resumo em MEMORY.md
5. `/budget 1000000` atualiza limite em runtime e persiste em `.claw/budget.json`
6. Restart sem flags carrega budget de `.claw/budget.json`
7. `--no-budget` ignora tudo e roda sem limites

---

## Ordem de Implementacao Recomendada

1. Step 1 (budget.rs) — fundacao, pode ser testado isoladamente
2. Step 6 (persistencia) — parte de budget.rs, testar round-trip
3. Step 2 (CLI flags) — conecta config ao startup
4. Step 4 (/budget slash) — permite controle em runtime
5. Step 3 (TUI integration) — visual feedback
6. Step 5 (graceful save) — safety net final
