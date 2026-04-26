# Plan: Comando `dream` (compressao de memoria)

**Data:** 2026-04-26
**Complexidade:** MEDIUM
**Escopo:** 4 arquivos modificados, 1 arquivo novo

---

## Contexto

O projeto elai-code (CLI de IA em Rust) precisa de um comando `/dream` que comprime entradas antigas do arquivo de memoria (ELAI.md / CLAUDE.md) usando o modelo de IA para sumarizar. A referencia TypeScript (`mythos-router/src/commands/dream.ts`) mostra o fluxo: ler MEMORY.md, separar as 20 entradas mais recentes (manter intactas), enviar as mais antigas ao modelo para sumarizacao, reescrever o arquivo com `[COMPRESSED SUMMARY]` + 20 entradas recentes.

**Diferenca chave vs TypeScript:** No TS, o formato e uma tabela Markdown com `| Timestamp | Action | Result |`. No Rust/elai, o arquivo de memoria e ELAI.md (Markdown livre, nao tabular). O parsing precisa adaptar-se a secoes Markdown (`## heading` ou `---` como separadores de entradas).

### Arquitetura existente relevante

- **SlashCommand enum** em `crates/commands/src/lib.rs` (linhas 253-324) -- onde adicionar `Dream { force: bool }`
- **SlashCommand::parse()** em `crates/commands/src/lib.rs` (linhas 326-411) -- onde adicionar `"dream"` match arm
- **SLASH_COMMAND_SPECS** em `crates/commands/src/lib.rs` (linhas 51-250) -- onde registrar spec
- **handle_repl_command()** em `crates/elai-cli/src/main.rs` (~linha 1710) -- onde rotear `SlashCommand::Dream`
- **handle_tui_slash_command()** em `crates/elai-cli/src/main.rs` (~linha 1300) -- onde rotear na TUI
- **resume_command()** em `crates/elai-cli/src/main.rs` (~linha 917) -- onde adicionar ao match
- **run_internal_prompt_text()** em `crates/elai-cli/src/main.rs` (~linha 2158) -- padrao para chamar a API com tools=false (bughunter, commit, pr usam isso)
- **ProviderClient::send_message()** em `crates/api/src/client.rs` -- API subjacente
- **Arquivos de memoria**: `ELAI.md`, `ELAI.local.md`, `.elai/ELAI.md`, `.elai/instructions.md` -- descobertos por `discover_instruction_files()` em `crates/runtime/src/prompt.rs`

---

## Guardrails

### MUST HAVE
- Backup do arquivo original antes de qualquer reescrita (`.bak`)
- Protecao: se o arquivo tem <= 20 secoes/entradas, nao comprimir (nada a fazer)
- Flag `--force` para bypass do minimo de entradas
- Saida com progresso: quantas entradas comprimidas, tamanho antes/depois em chars
- Testes unitarios para parsing e protecao

### MUST NOT
- Nao alterar a logica de `discover_instruction_files` no runtime crate
- Nao adicionar dependencias externas novas ao elai-cli (ja tem tokio, api, etc.)
- Nao compactar arquivos que nao sao do projeto atual (nao tocar `~/.elai/`)

---

## Task Flow

### Step 1: Registrar `SlashCommand::Dream` no crate `commands`

**Arquivo:** `crates/commands/src/lib.rs`

**Modificacoes:**

1. Adicionar ao `SLASH_COMMAND_SPECS`:
   ```rust
   SlashCommandSpec {
       name: "dream",
       aliases: &[],
       summary: "Compress old memory entries into a summary",
       argument_hint: Some("[--force]"),
       resume_supported: false,
   },
   ```

2. Adicionar variante ao `enum SlashCommand`:
   ```rust
   Dream { force: bool },
   ```

3. Adicionar ao `SlashCommand::parse()` match:
   ```rust
   "dream" => Self::Dream {
       force: parts.next() == Some("--force"),
   },
   ```

**Criterio de aceitacao:** `SlashCommand::parse("/dream")` retorna `Some(SlashCommand::Dream { force: false })` e `SlashCommand::parse("/dream --force")` retorna `Some(SlashCommand::Dream { force: true })`. Aparece no `/help`.

---

### Step 2: Criar modulo `dream.rs` com logica principal

**Arquivo novo:** `crates/elai-cli/src/dream.rs`

**Structs e funcoes:**

```rust
/// Resultado do parsing do arquivo de memoria
pub struct MemoryParseResult {
    pub old_entries: Vec<String>,     // entradas para comprimir
    pub recent_entries: Vec<String>,  // ultimas 20 entradas (intactas)
    pub existing_summary: Option<String>, // summary anterior se existir
}

/// Resultado da compressao
pub struct DreamResult {
    pub entries_compressed: usize,
    pub before_size: usize,
    pub after_size: usize,
    pub summary: String,
}
```

**Funcoes:**

1. **`fn find_memory_file(cwd: &Path) -> Option<PathBuf>`**
   - Procura `ELAI.md`, `CLAUDE.md`, `.elai/ELAI.md`, `.elai/instructions.md` (somente no CWD, nao ancestrais)
   - Retorna o primeiro que existir

2. **`fn parse_memory_sections(content: &str) -> MemoryParseResult`**
   - Divide o conteudo em entradas usando `## ` headings ou `---` horizontal rules como separadores
   - Se existir um bloco `<!-- [COMPRESSED SUMMARY] -->` no inicio, extrai como `existing_summary`
   - Separa as ultimas 20 entradas como `recent_entries`, o restante como `old_entries`
   - Se total de entradas <= 20, `old_entries` fica vazio

3. **`fn build_compression_prompt(entries: &[String], existing_summary: Option<&str>) -> String`**
   - Monta o prompt para o modelo (adaptado do TS):
     - System: "You are a memory compression engine. Output only the summary, nothing else."
     - User: instrucoes para preservar decisoes arquiteturais, arquivos modificados, erros/correcoes, trajetoria geral
     - Se ja existe um summary anterior, inclui-lo como contexto adicional para merge

4. **`fn rewrite_memory(path: &Path, summary: &str, recent: &[String]) -> io::Result<()>`**
   - Copia `path` para `path.with_extension("md.bak")` (backup)
   - Reescreve o arquivo com formato:
     ```
     <!-- [COMPRESSED SUMMARY] -->
     {summary}
     <!-- [/COMPRESSED SUMMARY] -->

     {recent entries joined by \n\n---\n\n or original separators}
     ```

5. **`fn format_dream_output(result: &DreamResult) -> String`**
   - Formata a saida do terminal com stats de compressao

**Criterio de aceitacao:**
- `parse_memory_sections` com conteudo de 30 secoes retorna 10 em `old_entries` e 20 em `recent_entries`
- `parse_memory_sections` com conteudo de 15 secoes retorna 0 em `old_entries` e 15 em `recent_entries`
- `rewrite_memory` cria arquivo `.bak` identico ao original
- O arquivo reescrito contem o marcador `[COMPRESSED SUMMARY]` e as entradas recentes

---

### Step 3: Integrar no REPL e handler de commands

**Arquivo:** `crates/elai-cli/src/main.rs`

**Modificacoes:**

1. Adicionar `mod dream;` no topo (junto com `mod init;`, `mod input;`, etc.)

2. No `handle_repl_command()` (~linha 1710), adicionar arm:
   ```rust
   SlashCommand::Dream { force } => {
       self.run_dream(force)?;
       false
   }
   ```

3. Implementar `fn run_dream(&self, force: bool)` em `impl LiveCli`:
   - Chamar `dream::find_memory_file(&cwd)` -- se None, imprimir erro e retornar
   - Ler o arquivo, chamar `dream::parse_memory_sections(&content)`
   - Verificar protecao: se `old_entries.is_empty()` e `!force`, imprimir aviso e retornar
   - Imprimir progresso: "Dreaming... compressing {n} entries"
   - Montar prompt com `dream::build_compression_prompt()`
   - Chamar `self.run_internal_prompt_text(&prompt, false)` (sem tools, igual ao /commit)
   - Chamar `dream::rewrite_memory(path, &summary, &recent_entries)`
   - Imprimir resultado com `dream::format_dream_output()`

4. No `resume_command()` (~linha 917), adicionar ao match `SlashCommand::Dream { .. }` no grupo de "unsupported resumed" (dream precisa de API, nao faz sentido em resume)

**Criterio de aceitacao:**
- `/dream` no REPL executa sem panic, imprime progresso, e reescreve o arquivo
- `/dream` com arquivo de memoria com <= 20 entradas imprime aviso e nao modifica nada
- `/dream --force` bypassa a protecao

---

### Step 4: Integrar na TUI

**Arquivo:** `crates/elai-cli/src/main.rs`

**Modificacoes:**

1. No `handle_tui_slash_command()` (~linha 1300), adicionar arm `"dream"`:
   - Mostrar dialogo de confirmacao: "Compress {n} old entries? This rewrites your memory file. (y/N)"
   - Se confirmado, executar a mesma logica do REPL (via funcao compartilhada)
   - Postar resultado como `ChatEntry::SystemNote`

2. Adicionar `/dream` a help string da TUI (~linha 1330):
   ```
   /dream         Comprimir entradas antigas da memoria (AI summary)\n\
   ```

**Criterio de aceitacao:**
- `/dream` na TUI mostra nota de confirmacao antes de executar
- Resultado aparece como SystemNote no chat

---

### Step 5: Testes

**Arquivo:** `crates/elai-cli/src/dream.rs` (section `#[cfg(test)]`)

**Testes:**

1. **`test_parse_sections_with_headings`**: conteudo com 25 `## Section N` -> 5 old, 20 recent
2. **`test_parse_sections_with_hr_separators`**: conteudo com 25 `---` blocos -> 5 old, 20 recent
3. **`test_parse_sections_few_entries`**: conteudo com 10 entradas -> 0 old, 10 recent
4. **`test_parse_preserves_existing_summary`**: conteudo com `<!-- [COMPRESSED SUMMARY] -->` -> extraido corretamente
5. **`test_rewrite_creates_backup`**: usa tempdir, escreve arquivo, chama rewrite, verifica .bak existe e conteudo identico
6. **`test_rewrite_output_format`**: verifica que arquivo reescrito contem marcadores e entradas recentes
7. **`test_build_compression_prompt`**: verifica que prompt contem instrucoes chave e entradas

**Criterio de aceitacao:** `cargo test -p elai-cli -- dream` passa todos os 7 testes.

---

## Arquivos: Resumo

| Acao     | Arquivo                              | O que muda                                       |
|----------|--------------------------------------|--------------------------------------------------|
| MODIFICAR | `crates/commands/src/lib.rs`        | +SlashCommandSpec, +enum variant, +parse arm     |
| CRIAR    | `crates/elai-cli/src/dream.rs`       | Modulo novo com toda a logica de dream            |
| MODIFICAR | `crates/elai-cli/src/main.rs`       | +mod dream, +REPL handler, +TUI handler, +help   |
| MODIFICAR | `crates/elai-cli/Cargo.toml`        | Nenhuma dependencia nova necessaria (tempfile para testes: use std::fs) |

---

## Success Criteria

1. `elai /dream` no modo REPL comprime entradas antigas de ELAI.md usando a API do modelo
2. `/dream` na TUI pede confirmacao antes de executar
3. Arquivo original e preservado como `.bak`
4. Com <= 20 entradas, o comando nao faz nada (a nao ser com `--force`)
5. Saida mostra: entradas comprimidas, tamanho antes/depois, summary gerado
6. Todos os testes passam: `cargo test -p elai-cli -- dream`
7. `cargo clippy --workspace --all-targets -- -D warnings` limpo
