# Plan: Comando `verify` -- Sync Codebase x Memoria

**Data:** 2026-04-26
**Complexidade:** MEDIUM
**Escopo:** 5 arquivos modificados, 1 novo arquivo, ~400 linhas de codigo novo

---

## Contexto

O projeto claw-cli ja possui o comando `/memory` que exibe arquivos CLAW.md descobertos. O `verify` vai alem: percorre a arvore do projeto (respeitando .gitignore), extrai paths mencionados nos arquivos de instrucao (CLAW.md, CLAUDE.md, .claw/memory.md), e compara os dois conjuntos para categorizar cada entrada como verified, missing, untracked ou drift.

Referencia TypeScript: `mythos-router/src/commands/verify.ts` -- usa walkDirectory com depth limit=10, ignorePatterns simples (nome ou extensao), e extrai paths via regex `CREATE|MODIFY|DELETE|READ|chat:` do MEMORY.md.

No claw-cli, a "memoria" sao os instruction files descobertos por `ProjectContext::discover()` em `crates/runtime/src/prompt.rs`. Os paths mencionados neles serao comparados contra o filesystem real.

---

## Objetivos

1. `claw verify` como subcomando CLI (nao-interativo, saida direta no terminal)
2. `/verify` como slash command na TUI e no runtime REPL
3. Saida colorida em tabela ASCII com contadores sumarios
4. Sem dependencias externas novas (sem crate `ignore` ou `globset`) -- walker proprio lendo .gitignore manualmente

---

## Guardrails

### Must Have
- Respeitar .gitignore do projeto (parse basico: linhas nao-comentario, nao-vazias)
- Ignorar sempre: `.git/`, `target/`, `node_modules/`, `.DS_Store`, `.omc/`
- Depth limit: 15 niveis de recursao (safety)
- Categorias: verified, missing, untracked, drift
- Testes com diretorio temporario e CLAW.md simulado

### Must NOT Have
- TUI full-screen para o verify (apenas output de texto)
- Dependencia em crates externas para walk/gitignore
- Modificacao do CLAW.md (verify e read-only)
- Reescrita da logica de `ProjectContext::discover`

---

## Task Flow

### Step 1: Criar `crates/claw-cli/src/verify.rs` (novo modulo)

**Arquivo:** `crates/claw-cli/src/verify.rs` (NOVO)

**Structs e tipos:**

```rust
/// Uma entrada extraida dos instruction files com path mencionado
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub path: PathBuf,          // path relativo encontrado no CLAW.md
    pub source_file: PathBuf,   // qual instruction file mencionou
    pub line_number: usize,     // linha onde foi encontrado
}

/// Resultado da verificacao
#[derive(Debug, Default)]
pub struct VerifyReport {
    pub verified: Vec<PathBuf>,    // no disco E na memoria
    pub missing: Vec<PathBuf>,     // na memoria mas NAO no disco
    pub untracked: Vec<PathBuf>,   // no disco mas NAO na memoria
    pub drift: Vec<PathBuf>,       // existe mas contexto sugere mudanca (DELETE mencionado mas arquivo presente, etc)
    pub files_scanned: usize,
    pub memory_entries: usize,
}
```

**Funcoes publicas:**

```rust
/// Percorre arvore do projeto respeitando .gitignore e hardcoded ignores
pub fn walk_project(root: &Path) -> io::Result<Vec<PathBuf>>

/// Extrai paths mencionados nos instruction files (CLAW.md, etc)
pub fn parse_memory_entries(instruction_files: &[ContextFile]) -> Vec<MemoryEntry>

/// Compara filesystem vs memoria e gera o report
pub fn diff_entries(
    root: &Path,
    files: &[PathBuf],
    memory: &[MemoryEntry],
) -> VerifyReport

/// Renderiza o report como string colorida para o terminal
pub fn render_verify_report(report: &VerifyReport, root: &Path) -> String

/// Ponto de entrada: executa verify completo e retorna string formatada
pub fn run_verify() -> Result<String, Box<dyn std::error::Error>>
```

**Logica de `walk_project`:**
1. Ler `.gitignore` na raiz (se existir) -- linhas nao-vazias, sem `#`
2. Merge com hardcoded: `[".git", "target", "node_modules", ".DS_Store", ".omc", ".claw/sessions"]`
3. Recursao `walk_inner(dir, root, &patterns, depth)` com depth max 15
4. Para cada entry: se nome bate com pattern (exato ou `*.ext`), skip. Se hidden (`.` prefix) e nao explicitamente listado, skip.
5. Retornar Vec<PathBuf> com paths relativos ao root

**Logica de `parse_memory_entries`:**
1. Para cada `ContextFile` nos instruction_files:
2. Para cada linha, buscar patterns via regex simples (sem crate regex -- usar `str::contains` e `str::split`):
   - Linhas que contem paths com extensao reconhecida (`.rs`, `.ts`, `.toml`, `.md`, `.json`, `.yaml`, `.yml`, `.py`, `.go`, `.sh`)
   - Linhas com patterns `CREATE:`, `MODIFY:`, `DELETE:`, `READ:`, `file:`, ou backtick paths como `` `src/foo.rs` ``
   - Paths tipo `crates/foo/bar.rs`, `src/main.rs`, etc (contem `/` e extensao)
3. Normalizar paths relativos, deduplicar

**Logica de `diff_entries`:**
1. Converter files para Set<PathBuf> (relativos)
2. Converter memory paths para Set<PathBuf>
3. `verified` = interseccao (no disco E na memoria)
4. `missing` = na memoria mas nao no disco
5. `untracked` = no disco mas nao na memoria
6. `drift` = arquivos que existem mas memoria sugere DELETE

**Logica de `render_verify_report`:**
- Usar ANSI codes diretamente (o projeto ja usa crossterm, mas para saida simples basta `\x1b[32m` etc)
- Formato:
```
Verify -- Codebase x Memory Sync
  Scanned 142 files
  Memory has 23 entries

File References in Memory:
  OK  src/main.rs (4.2KB)
  OK  Cargo.toml (312B)
  !!  src/deleted.rs -- missing from filesystem
  ??  src/old.rs -- drift (memory says DELETE)

Untracked Files (not in memory):
  .  src/new_feature.rs (1.1KB)
  .  tests/integration.rs (892B)
  ... and 118 more

Summary: 20 verified | 2 drift | 1 missing | 119 untracked
```

**Acceptance Criteria:**
- [x] `walk_project` retorna lista de arquivos respeitando .gitignore
- [x] `parse_memory_entries` extrai paths de instruction files
- [x] `diff_entries` categoriza corretamente em 4 buckets
- [x] `render_verify_report` gera string legivel com cores ANSI
- [x] `run_verify` orquestra tudo e retorna Result<String>

---

### Step 2: Integrar como subcomando CLI (`claw verify`)

**Arquivos a modificar:**

1. **`crates/claw-cli/src/main.rs`**
   - Adicionar `mod verify;` no topo (junto com `mod init; mod swd;`)
   - Adicionar variante `Verify` ao `enum CliAction` (linha ~168)
   - Adicionar match arm em `run()` (linha ~71): `CliAction::Verify => run_verify_command()`
   - Adicionar case `"verify"` no `match rest[0].as_str()` (linha ~361, junto com "login", "logout", etc)
   - Adicionar funcao `fn run_verify_command()` que chama `verify::run_verify()` e imprime resultado

2. **`crates/claw-cli/src/args.rs`**
   - Adicionar `Verify` ao `enum Command` com doc comment `/// Verify codebase vs memory sync`
   - Adicionar test case em `parses_login_and_logout_commands` ou novo test

**Acceptance Criteria:**
- [x] `claw verify` executa e imprime report
- [x] `claw --help` lista verify como subcomando disponivel
- [x] Exit code 0 se tudo ok, 1 se drift/missing encontrado

---

### Step 3: Integrar como slash command (`/verify`)

**Arquivos a modificar:**

1. **`crates/commands/src/lib.rs`**
   - Adicionar `SlashCommandSpec` para "verify" (junto com os outros, ~linha 128):
     ```rust
     SlashCommandSpec {
         name: "verify",
         aliases: &[],
         summary: "Verify codebase files against memory entries",
         argument_hint: None,
         resume_supported: true,
     },
     ```
   - Adicionar variante `Verify` ao `enum SlashCommand` (linha ~302, junto com Memory, Init, etc)
   - Adicionar case `"verify" => Self::Verify` no `parse()` (linha ~385)
   - Adicionar `SlashCommand::Verify` aos patterns de `resume_supported` (linha ~1634 area)

2. **`crates/claw-cli/src/main.rs`**
   - Adicionar handler no `resume_slash_command` match (linha ~989 area):
     ```rust
     SlashCommand::Verify => Ok(ResumeCommandOutcome {
         session: session.clone(),
         message: Some(verify::run_verify()?),
     }),
     ```
   - Adicionar case `"verify"` no TUI slash dispatch (linha ~1463 area):
     ```rust
     "verify" => {
         match verify::run_verify() {
             Ok(report) => app.push_chat(tui::ChatEntry::SystemNote(report)),
             Err(e) => app.push_chat(tui::ChatEntry::SystemNote(format!("Erro: {e}"))),
         }
     }
     ```

3. **`crates/claw-cli/src/tui.rs`**
   - Adicionar `("verify".into(), "Verificar codebase vs memoria".into())` na lista de comandos (linha ~443)

4. **`crates/claw-cli/src/main.rs`** -- help text
   - Adicionar `/verify` na string de help da TUI (linha ~1334):
     `  /verify        Verificar codebase vs memoria (CLAW.md)\n\`

**Acceptance Criteria:**
- [x] `/verify` funciona no REPL e na TUI
- [x] Aparece no `/help` e na paleta de comandos (Ctrl+K)
- [x] Output e exibido como SystemNote no chat

---

### Step 4: Leitura de .gitignore sem dependencia externa

**Dentro de `verify.rs`, funcao auxiliar:**

```rust
struct IgnoreRules {
    patterns: Vec<IgnorePattern>,
}

enum IgnorePattern {
    Exact(String),           // "target", ".git"
    Extension(String),       // "*.o" -> ".o"
    PathPrefix(String),      // "build/" -> "build"
    Negation(String),        // "!important.txt" (nao ignorar)
}

impl IgnoreRules {
    fn load(root: &Path) -> Self { ... }
    fn should_ignore(&self, name: &str, rel_path: &Path) -> bool { ... }
}
```

**Logica de parse:**
1. Ler `root/.gitignore` se existir
2. Cada linha: trim, skip `#` e vazia
3. Se comeca com `!` -> negation
4. Se comeca com `*.` -> extension
5. Se termina com `/` -> directory match (PathPrefix)
6. Senao -> exact name match
7. Merge com hardcoded defaults

**Acceptance Criteria:**
- [x] Respeita `.gitignore` do projeto
- [x] Hardcoded defaults sempre aplicados (`.git`, `target`, `node_modules`)
- [x] Linhas com `!` sao tratadas como negacao
- [x] Nenhum crate externo para gitignore

---

### Step 5: Testes

**Arquivo:** `crates/claw-cli/src/verify.rs` -- secao `#[cfg(test)] mod tests`

**Casos de teste:**

1. **`test_walk_project_respects_gitignore`**
   - Cria tmpdir com .gitignore contendo "*.log"
   - Cria arquivos: `src/main.rs`, `debug.log`, `target/build`
   - Verifica: main.rs presente, debug.log ausente, target/ ausente

2. **`test_parse_memory_entries_extracts_paths`**
   - Cria ContextFile com conteudo tipo CLAW.md mencionando `src/main.rs`, `Cargo.toml`
   - Verifica extracoes corretas e deduplicacao

3. **`test_diff_entries_categorizes_correctly`**
   - files = [a.rs, b.rs, c.rs]
   - memory = [a.rs, b.rs, d.rs]
   - verified = [a.rs, b.rs], untracked = [c.rs], missing = [d.rs]

4. **`test_ignore_rules_parse`**
   - Testa parse de `.gitignore` com patterns mistos
   - Testa negation patterns

5. **`test_render_verify_report_format`**
   - VerifyReport com dados conhecidos
   - Verifica que output contem "verified", "missing", contadores corretos

6. **`test_walk_project_depth_limit`**
   - Cria diretorio com 20 niveis de nesting
   - Verifica que walk para em 15

**Acceptance Criteria:**
- [x] Todos os 6 testes passam com `cargo test -p claw-cli`
- [x] Testes usam tmpdir e fazem cleanup
- [x] Nenhum teste depende de estado externo

---

## Arquivos a Criar/Modificar

| Arquivo | Acao | Descricao |
|---------|------|-----------|
| `crates/claw-cli/src/verify.rs` | CRIAR | Modulo principal: walker, parser, diff, render |
| `crates/claw-cli/src/main.rs` | MODIFICAR | `mod verify`, CliAction::Verify, handlers TUI + REPL |
| `crates/claw-cli/src/args.rs` | MODIFICAR | Command::Verify no enum clap |
| `crates/claw-cli/src/tui.rs` | MODIFICAR | Adicionar verify na paleta de comandos |
| `crates/commands/src/lib.rs` | MODIFICAR | SlashCommandSpec + SlashCommand::Verify + parse |

---

## Criterios de Sucesso

1. `claw verify` imprime report correto num projeto real
2. `/verify` funciona em REPL e TUI
3. .gitignore do projeto e respeitado
4. Zero dependencias novas no Cargo.toml
5. `cargo test -p claw-cli` passa incluindo os novos testes
6. `cargo clippy -p claw-cli` sem warnings
