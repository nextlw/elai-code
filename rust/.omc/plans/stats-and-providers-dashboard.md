# Plan: Comando `stats` e Dashboard de Providers

**Data:** 2026-04-26
**Complexidade estimada:** MEDIUM
**Escopo:** ~6 arquivos a criar/modificar em 2 crates (`commands`, `elai-cli`)

---

## Contexto

O projeto elai-cli usa um modelo de comandos duplo:
1. **CLI subcommands** via `CliAction` enum em `main.rs` (parsed manualmente por `parse_args`)
2. **Slash commands** via `SlashCommand` enum em `crates/commands/src/lib.rs` (parsing por `SlashCommand::parse`, dispatch em `LiveCli`)

Sessoes ficam em `.elai/sessions/{id}.json` (formato `Session` do crate `runtime`).
O crate `runtime::usage` ja tem `TokenUsage`, `ModelPricing`, `pricing_for_model()`, `UsageCostEstimate` e `format_usd()` prontos para reuso.

Nao existe telemetria persistida por-request hoje. O `UsageTracker` e in-memory e por-sessao. Portanto, **o primeiro passo e definir o formato de telemetria JSONL** que sera a fonte de dados para `stats`.

A referencia TypeScript (`mythos-router/src/commands/stats.ts`) le um `metrics.json` com entries contendo `{ timestamp, command, project, costUSD, inputTokens, outputTokens, turns }` e agrupa por comando/projeto. A `providers.ts` le telemetria de providers com score EMA, latencia, success rate e decisoes de routing recentes.

---

## Guardrails

### MUST HAVE
- Zero dependencias externas novas para formatacao de tabela (ASCII manual)
- Compativel com o sistema de slash commands existente (`SlashCommandSpec` + `SlashCommand` enum)
- Reusar `pricing_for_model()` e `TokenUsage` de `runtime::usage`
- Testes unitarios para aggregation, filtro de dias e formatacao

### MUST NOT
- Nao adicionar crate `tabled`, `comfy-table` ou similar
- Nao alterar o formato de `Session` existente
- Nao quebrar os testes existentes de `args.rs` e `commands/src/lib.rs`
- Dashboard de providers NAO depende de ratatui neste plano (sera output textual como os demais slash commands; integracao TUI/ratatui e escopo futuro)

---

## Task Flow

### Step 1 -- Definir formato de telemetria JSONL e writer

**Arquivos a criar:**
- `crates/runtime/src/telemetry.rs` (novo)

**Structs e funcoes:**

```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEntry {
    pub timestamp: String,          // ISO 8601 UTC
    pub session_id: String,
    pub project: String,            // basename do cwd
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub success: bool,
    pub provider: Option<String>,   // para dashboard de providers futuro
    pub error_type: Option<String>,
}

pub struct TelemetryWriter { path: PathBuf }

impl TelemetryWriter {
    pub fn new(path: PathBuf) -> Self;
    pub fn append(&self, entry: &TelemetryEntry) -> io::Result<()>;  // append JSONL line
}

pub fn default_telemetry_path() -> PathBuf;  // ~/.elai/telemetry.jsonl

pub fn load_entries(path: &Path, since: Option<chrono_free_cutoff_secs>) -> io::Result<Vec<TelemetryEntry>>;
// Nota: "since" e um unix timestamp i64; calcular cutoff como:
//   SystemTime::now() - Duration::from_secs(days * 86400)
// Parsear timestamp ISO 8601 manualmente com um helper simples
//   (ou aceitar apenas o formato fixo "2026-04-26T12:00:00Z")
```

**Modificar:** `crates/runtime/src/lib.rs` -- adicionar `pub mod telemetry;`

**Acceptance criteria:**
- `TelemetryWriter::append` escreve uma linha JSON valida + `\n`
- `load_entries` retorna todas as entries; com `since`, filtra por timestamp
- Teste: write 3 entries, load com filtro de 1 dia, verificar count

---

### Step 2 -- Emitir telemetria no loop de request do CLI

**Arquivo a modificar:** `crates/elai-cli/src/main.rs`

**O que fazer:**
- Apos cada resposta da API (onde `TokenUsage` e extraido), chamar `TelemetryWriter::append` com os dados do turn
- Instanciar `TelemetryWriter` em `LiveCli::new()` (ou equivalente) usando `default_telemetry_path()`
- Capturar: model, project (basename de cwd), session_id, tokens, custo via `pricing_for_model`, latencia (Instant antes/depois do request)

**Acceptance criteria:**
- Apos um `elai "hello"`, o arquivo `~/.elai/telemetry.jsonl` contem pelo menos 1 linha valida
- Entry contem todos os campos obrigatorios

---

### Step 3 -- Comando `stats` (CLI subcommand + slash command)

**Arquivos a criar:**
- `crates/commands/src/stats.rs` (novo) -- logica de aggregation e render

**Arquivos a modificar:**
- `crates/commands/src/lib.rs` -- adicionar `pub mod stats;`, novo `SlashCommandSpec`, novo variante `SlashCommand::Stats`
- `crates/elai-cli/src/main.rs` -- adicionar `CliAction::Stats` e dispatch para `parse_args`, adicionar handler no slash command match

**Structs e funcoes em `stats.rs`:**

```
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct AggregatedStats {
    pub requests: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
}

/// Agrupa entries por chave (model, project, etc)
pub fn aggregate_by<F>(entries: &[TelemetryEntry], key_fn: F) -> BTreeMap<String, AggregatedStats>
where
    F: Fn(&TelemetryEntry) -> String;

/// Calcula overall totals
pub fn overall_stats(entries: &[TelemetryEntry]) -> AggregatedStats;

/// Renderiza tabela ASCII alinhada.
/// Colunas: Key | Requests | Input Tok | Output Tok | Cost USD | Avg Latency
/// Alinhamento: calcular max width de cada coluna iterando os dados primeiro.
pub fn render_stats_table(title: &str, stats: &BTreeMap<String, AggregatedStats>) -> String;

/// Renderiza o report completo (overall + by-model + by-project)
pub fn render_stats_report(
    entries: &[TelemetryEntry],
    by_model: bool,
    by_project: bool,
    days: Option<u32>,
) -> String;
```

**Formatacao de tabela ASCII (sem dependencia externa):**

```
fn render_stats_table(...) -> String {
    // 1. Coletar todas as rows como Vec<[String; 6]>
    // 2. Para cada coluna, calcular max(header.len(), max(row[col].len()))
    // 3. Header: pad cada celula com format!("{:<width$}", val)
    // 4. Separador: "-".repeat(total_width)
    // 5. Rows: mesma logica de pad, numeros alinhados a direita com format!("{:>width$}", val)
    // 6. Footer com total
}
```

**CLI args:** `elai stats [--days N] [--by-model] [--by-project]`
**Slash command:** `/stats [--days N]` (by-model e default dentro da TUI)

**CliAction::Stats:**
```
Stats {
    days: Option<u32>,
    by_model: bool,
    by_project: bool,
}
```

**SlashCommand::Stats:**
```
Stats {
    days: Option<u32>,
}
```

**Acceptance criteria:**
- `elai stats` imprime tabela com overall + by-model
- `elai stats --days 7` filtra ultimos 7 dias
- `elai stats --by-project` mostra breakdown por projeto
- `/stats` na TUI funciona e imprime como system message
- Tabela alinha corretamente com dados de largura variavel

---

### Step 4 -- Comando `providers` (slash command)

**Arquivos a criar:**
- `crates/commands/src/providers.rs` (novo) -- logica de dashboard de providers

**Arquivos a modificar:**
- `crates/commands/src/lib.rs` -- adicionar `pub mod providers;`, novo `SlashCommandSpec`, novo variante `SlashCommand::Providers`
- `crates/elai-cli/src/main.rs` -- handler no slash command match

**Structs e funcoes em `providers.rs`:**

```
#[derive(Debug, Clone)]
pub struct ProviderHealthSummary {
    pub id: String,
    pub total_calls: u32,
    pub success_rate: f64,       // 0.0..1.0
    pub avg_latency_ms: f64,
    pub ema_latency_ms: f64,     // EMA com alpha=0.3
    pub score: f64,              // success_rate * 100 - avg_latency * 0.05
    pub status: ProviderStatus,
}

#[derive(Debug, Clone, Copy)]
pub enum ProviderStatus {
    Healthy,
    Degraded { reason: &'static str },
}

pub fn confidence_label(total_calls: u32) -> &'static str;
// <20 = "Low", 20..=100 = "Medium", >100 = "High"

/// Calcula EMA de latencia a partir das entries do provider (mais recentes primeiro)
pub fn compute_ema_latency(latencies: &[f64], alpha: f64) -> f64;

/// Agrega telemetria por provider
pub fn aggregate_providers(entries: &[TelemetryEntry]) -> Vec<ProviderHealthSummary>;

/// Extrai as N decisoes de routing mais recentes (entries com provider preenchido, ordenadas por timestamp desc)
pub fn recent_routing_decisions(entries: &[TelemetryEntry], limit: usize) -> Vec<&TelemetryEntry>;

/// Extrai falhas recentes (success=false)
pub fn recent_failures(entries: &[TelemetryEntry], limit: usize) -> Vec<&TelemetryEntry>;

/// Renderiza o dashboard completo como String (tabela ASCII)
pub fn render_providers_dashboard(entries: &[TelemetryEntry], verbose: bool) -> String;
```

**Layout do dashboard:**

```
Provider Health & Orchestration State
──────────────────────────────────────────────────
  Leader: anthropic (Score: 95.2) [High Confidence]
──────────────────────────────────────────────────
  Provider       | Status          | EMA Latency | Success Rate
  ─────────────────────────────────────────────────────────────
  anthropic      | Healthy         | 320ms       | 99.2%
  openai         | Healthy         | 450ms       | 97.8%
  xai            | Degraded (Low%) | 890ms       | 82.1%
──────────────────────────────────────────────────
  Recent Routing Decisions
  [14:32:05] anthropic chosen for task (1.2k tokens)
  [14:31:42] openai chosen for task (800 tokens)
──────────────────────────────────────────────────
  Recent Failures
  [14:30:10] xai | RateLimit | 429 Too Many Requests
```

**Slash command:** `/providers [--verbose]`

**Acceptance criteria:**
- `/providers` imprime tabela de providers com score, latencia EMA, success rate
- Providers sem dados mostra mensagem "No provider telemetry found yet"
- Score calculado corretamente: `success_rate * 100 - avg_latency * 0.05`
- EMA calculado com alpha=0.3 sobre as latencias ordenadas cronologicamente

---

### Step 5 -- Testes

**Arquivos a criar/modificar:**
- Testes inline em `crates/runtime/src/telemetry.rs` (`#[cfg(test)] mod tests`)
- Testes inline em `crates/commands/src/stats.rs` (`#[cfg(test)] mod tests`)
- Testes inline em `crates/commands/src/providers.rs` (`#[cfg(test)] mod tests`)

**Cobertura minima:**

| Modulo | Teste | O que valida |
|--------|-------|-------------|
| telemetry | `write_and_read_entries` | Write 3 entries, read all, assert count=3 |
| telemetry | `filter_by_days` | Write entries com timestamps de 1d e 10d atras, filtrar --days 3, assert count=1 |
| stats | `aggregate_by_model_groups_correctly` | 5 entries com 2 modelos, assert 2 groups com totais corretos |
| stats | `overall_stats_sums_all` | Assert soma de tokens e custo |
| stats | `render_table_aligns_columns` | Assert que todas as linhas tem o mesmo comprimento |
| stats | `empty_entries_no_panic` | Vec vazio retorna tabela so com header |
| providers | `ema_latency_computation` | Sequencia conhecida, assert EMA com tolerancia f64 |
| providers | `score_calculation` | Success=0.95, latency=400 -> score=95-20=75 |
| providers | `aggregate_groups_by_provider` | Entries com 2 providers, assert contagens |
| providers | `degraded_status_low_success` | Provider com success_rate < 0.85 -> Degraded |

**Acceptance criteria:**
- `cargo test -p runtime` passa (inclui telemetry tests)
- `cargo test -p commands` passa (inclui stats + providers tests)
- Todos os 10 testes listados existem e passam

---

## Arquivos -- Resumo

| Acao | Caminho |
|------|---------|
| CRIAR | `crates/runtime/src/telemetry.rs` |
| MODIFICAR | `crates/runtime/src/lib.rs` (add `pub mod telemetry`) |
| MODIFICAR | `crates/elai-cli/src/main.rs` (emit telemetry + CliAction::Stats + dispatch) |
| CRIAR | `crates/commands/src/stats.rs` |
| CRIAR | `crates/commands/src/providers.rs` |
| MODIFICAR | `crates/commands/src/lib.rs` (add mods, SlashCommandSpec, SlashCommand variants, parse/dispatch) |

---

## Decisoes tecnicas

1. **Sem chrono crate** -- timestamps ISO 8601 gerados com `SystemTime` + helper manual (ou `time` crate se ja presente no workspace). Parsing do filtro --days: converter para unix timestamp e comparar.

2. **Telemetria em `~/.elai/telemetry.jsonl`** (nao por-projeto) -- permite aggregation cross-project no `stats --by-project`. O campo `project` na entry identifica a origem.

3. **EMA com alpha=0.3** -- mesmo valor do TypeScript reference. Formula: `ema[0] = latency[0]; ema[i] = alpha * latency[i] + (1-alpha) * ema[i-1]`.

4. **`/providers` como slash command puro (texto)** -- nao como painel ratatui. Integracao com TUI popup e escopo separado, apos existir `ProviderOrchestrator` (plano 1 mencionado no prompt).

5. **Tabela ASCII manual** -- iterar dados 2x (1a para medir larguras, 2a para formatar). Numeros alinhados a direita, texto a esquerda. Sem box-drawing characters, apenas `-` e `|`.

---

## Success Criteria

- [ ] `elai stats` executa e imprime tabela formatada
- [ ] `elai stats --days 7 --by-model` filtra e agrupa corretamente
- [ ] `/stats` funciona como slash command na TUI
- [ ] `/providers` funciona como slash command na TUI
- [ ] `~/.elai/telemetry.jsonl` e populado apos cada request da API
- [ ] `cargo test -p runtime -p commands` passa sem regressoes
- [ ] Nenhuma dependencia nova adicionada aos Cargo.toml
