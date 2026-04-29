# Análise da Implementação de Batches de Tools — Elai Code TUI

## 📋 Visão Geral

A implementação de **batches de tools** no Elai Code é um sistema sofisticado de **agrupamento visual** e **gerenciamento de estado** para renderizar chamadas de ferramentas (tools) de forma organizada e responsiva na TUI.

### Objetivos principais:
1. **Agrupar chamadas de tools** executadas em sequência no mesmo bloco visual
2. **Diferenciar entre "pensamento" e "resposta"** do agente
3. **Renderizar estado em tempo real** com spinners, ✓ (ok), ✗ (erro)
4. **Manter análise narrativa** (frases do agente entre tools) em um batch separado

---

## 🏗️ Arquitetura de Dados

### 1. `BatchKind` — Tipo de bloco agrupado

```rust
pub enum BatchKind {
    Tools,      // Chamadas de ferramentas com status por item
    Analysis,   // Frases narrativas do agente (sem status, só ícone de fala)
}
```

**Propósito:**
- `Tools`: Renderiza items com ícones de status (⠋ spinner, ✓ ok, ✗ erro)
- `Analysis`: Renderiza frases do agente como "pensamentos transitórios" entre tools

---

### 2. `BatchItem` — Item dentro de um batch

```rust
pub struct BatchItem {
    pub label: String,      // Nome da tool (Tools) ou frase narrativa (Analysis)
    pub detail: String,     // Input resumido da tool (Tools) ou vazio (Analysis)
    pub status: ToolItemStatus,  // Running | Ok | Err
}
```

**Observações:**
- Para `Tools`: `label` = nome da tool, `detail` = primeiros 60 chars do input
- Para `Analysis`: `label` = frase narrativa, `detail` = "" (vazio)
- Status `Err` renderiza com cor de erro e ícone ✗

---

### 3. `BatchEntry` — Bloco no chat

```rust
pub enum ChatEntry {
    // ...
    BatchEntry {
        kind: BatchKind,       // Tools ou Analysis
        items: Vec<BatchItem>, // Itens em ordem cronológica (0 = mais antigo)
        closed: bool,          // true = não aceita novos items
    },
    // ...
}
```

**Estados:**
- `closed: false` → bloco vivo, aceita novos items do mesmo `kind`
- `closed: true` → bloco congelado, próximo item do mesmo tipo abre novo bloco

---

### 4. `BATCH_WINDOW` — Tamanho máximo

```rust
pub const BATCH_WINDOW: usize = 5;
```

**Comportamento:**
- Quando um batch ultrapassa 5 items, o mais antigo é **removido** (sliding window)
- Mantém a TUI compacta em conversas longas com muitas tools

---

## 🔄 Fluxo de Processamento de Mensagens

### Pipeline de um Turn (turno de conversa)

```
TextChunk → accumula em pending_narration
     ↓
[Opcional: TuiMsg::ToolCall]
     ↓
[Opcional: TuiMsg::ToolResult]
     ↓
[Opcional: TextChunk novamente]
     ↓
TuiMsg::Done → flush pending_narration, fecha batches
```

---

### Estados Internos da App

```rust
pub struct UiApp {
    pub pending_narration: String,  // Buffer de texto não-distribuído
    pub in_tool_chain: bool,        // true = estamos dentro de uma cadeia de tools
    // ... outros campos
}
```

**`pending_narration`:**
- Acumula `TextChunk` fragmentados da IA
- Só é "distribuído" ao chat quando:
  - Chega um `ToolCall` (decide: vai para Analysis ou AssistantText)
  - Chega `Done` (última linha promovida a AssistantText)

**`in_tool_chain`:**
- `true` = dentro de uma sequência de tool calls
- `false` = modo resposta textual normal
- Usado para decidir se texto intermediário é "pensamento" ou "resposta substantiva"

---

## 🎯 Heurística: "Pensamento" vs "Resposta Substantiva"

### Método: `pending_looks_like_substantial_response()`

```rust
fn pending_looks_like_substantial_response(&self) -> bool {
    let trimmed = self.pending_narration.trim();
    if trimmed.is_empty() {
        return false;
    }
    // 1. Parágrafo claro (linha em branco entre trechos)?
    if trimmed.contains("\n\n") {
        return true;
    }
    // 2. Múltiplas linhas não-vazias (lista, explicação)?
    let non_empty_lines = trimmed
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    non_empty_lines > 1
}
```

**Exemplos:**

| Texto | Resultado | Razão |
|-------|-----------|-------|
| `"Vou procurar."` | `false` | Linha única |
| `"Vou procurar.\n"` | `false` | Linha única + quebra final |
| `"Encontrei.\n- A\n- B"` | `true` | 2+ linhas não-vazias |
| `"Para:\n\nResponda."` | `true` | Contém `\n\n` (parágrafo) |

---

## 🎨 Renderização

### Ordem invertida dos items

```rust
// No rendering (chat_to_lines):
for item in items.iter().rev() {  // ← reversed!
    // render item
}
```

**Efeito visual:** Items mais recentes aparecem **no topo** do batch.

Exemplo (3 tools chronológicos: bash, ls, cat):
```
  ⚙ Tools (3)
    ✓ cat       ← mais recente (topo)
    ✓ ls
    ✓ bash      ← mais antigo (base)
```

---

### Renderização por `BatchKind`

#### Tools
```
  ⚙ Tools (N)
    ✓/✗/⠋ tool_name · first 60 chars of input
    ✓/✗/⠋ tool_name · first 60 chars of input
```

#### Analysis
```
  ⚙ Analise (N)
    💬 frase narrativa do agente
    💬 outra frase do agente
```

---

## 📊 Casos de Uso Detalhados

### Caso 1: Resposta Pura (sem tools)

```
Input: "Explique o projeto"
Output:
  TextChunk "Elai é..." → pending_narration += "Elai é..."
  TextChunk "Suporta..." → pending_narration += "Suporta..."
  Done → flush_pending_narration_as_assistant_text()

Resultado no Chat:
  ╭─ AssistantText ─────────────
  │ Elai é...Suporta...
  ╰──────────────────
```

**Chaves:**
- Nenhum `ToolCall` → `in_tool_chain` permanece `false`
- `Done` → `pending_narration` vira um único `AssistantText` (preserva markdown)

---

### Caso 2: Cadeia de Tools Única

```
Input: "Execute bash ls, pwd, whoami"

Tool Call 1 (ls)
  → append_batch_item(Tools, "bash" + "ls input")
Tool Result 1 (ok)
  → marca último item como Ok (✓)
Tool Call 2 (pwd)
  → append_batch_item(Tools, "bash" + "pwd input")  ← MESMO batch!
Tool Result 2 (ok)
  → marca último item como Ok (✓)
Tool Call 3 (whoami)
  → append_batch_item(Tools, "bash" + "whoami input")  ← MESMO batch!
Tool Result 3 (ok)
  → marca último item como Ok (✓)
Done
  → fecha o batch

Resultado no Chat:
  ⚙ Tools (3)
    ✓ bash · {"command":"whoami"}
    ✓ bash · {"command":"pwd"}
    ✓ bash · {"command":"ls"}
```

**Chaves:**
- `in_tool_chain = true` após 1º `ToolCall`
- Todos 3 `ToolCall` chegam sequencialmente → `append_batch_item()` anexa ao MESMO batch (pois é aberto e `kind == Tools`)

---

### Caso 3: Texto Intermediário Curto (Pensamento)

```
Input: "Procure o arquivo e relatar"

Tool Call 1 (bash find)
  → in_tool_chain = true
  → append_batch_item(Tools, "bash find")
Tool Result 1 (ok)
  → status = Ok
TextChunk "Encontrado. Vou examinar.\n"
  → pending_narration = "Encontrado. Vou examinar.\n"
Tool Call 2 (read_file)
  → pending_looks_like_substantial? NO (1 linha)
  → in_tool_chain = true ainda
  → flush_pending_narration_into_analysis(false)
     → cria BatchEntry { kind: Analysis, items: [item "Encontrado. Vou examinar."] }
  → append_batch_item(Tools, "read_file input")  ← NOVO batch Tools!

Resultado no Chat:
  ⚙ Tools (1)
    ✓ bash · {"command":"find"}
  
  ⚙ Analise (1)
    💬 Encontrado. Vou examinar.
  
  ⚙ Tools (1)
    ⠋ read_file · {"path":"..."}
```

**Chaves:**
- Texto curto entre tools → `pending_looks_like_substantial()` = false
- Vai para batch `Analysis`, não quebra o "modo tool"
- Próximo tool abre batch NOVO (pois `Analysis` interrompeu o agrupamento)

---

### Caso 4: Texto Intermediário Substantivo (Resposta)

```
Input: "Procure, analise, então diga o resultado"

Tool Call 1 (bash find)
Tool Result 1
Tool Call 2 (read_file)
Tool Result 2
TextChunk "Encontrei 3 padrões:\n- Padrão A...\n- Padrão B...\n- Padrão C..."
  → pending_looks_like_substantial()? YES (3+ linhas ou \n\n)
Tool Call 3 (write_file)
  → pending_looks_like_substantial? YES
  → in_tool_chain = true ainda, MAS:
  → flush_pending_narration_as_assistant_text()
     → cria ChatEntry::AssistantText("Encontrei 3 padrões...")
  → close_all_open_batches()  ← fecha Tools batch anterior
  → in_tool_chain = false  ← RESETA!
  → depois append_batch_item(Tools, ...)  ← NOVO batch

Done

Resultado no Chat:
  ⚙ Tools (2)
    ✓ read_file
    ✓ bash
  
  Encontrei 3 padrões:
  - Padrão A...
  - Padrão B...
  - Padrão C...
  
  ⚙ Tools (1)
    ⠋ write_file
```

**Chaves:**
- Texto "substantivo" (2+ linhas ou parágrafo) interrompe a cadeia
- Vira `AssistantText` (resposta da IA)
- Flag `in_tool_chain` reseta para `false`
- Próximo tool recomeça uma NOVA cadeia

---

## 🔧 Métodos-Chave

### `append_batch_item(kind, item)`

```rust
fn append_batch_item(&mut self, kind: BatchKind, item: BatchItem) {
    let can_append = matches!(
        self.chat.last(),
        Some(ChatEntry::BatchEntry { kind: k, closed: false, .. }) 
            if *k == kind  // ← MESMO tipo!
    );
    if can_append {
        if let Some(ChatEntry::BatchEntry { items, .. }) = self.chat.last_mut() {
            items.push(item);
            if items.len() > BATCH_WINDOW {
                items.remove(0);  // ← remove mais antigo
            }
        }
    } else {
        // Criar novo bloco
        self.chat.push(ChatEntry::BatchEntry {
            kind,
            items: vec![item],
            closed: false,
        });
    }
}
```

**Lógica:**
1. Se último entry = `BatchEntry` aberto do **MESMO `kind`** → anexa
2. Senão → cria novo bloco
3. Se excede `BATCH_WINDOW` → remove item mais antigo (posição 0)

---

### `close_all_open_batches()`

```rust
fn close_all_open_batches(&mut self) {
    for entry in self.chat.iter_mut().rev().take(4) {
        if let ChatEntry::BatchEntry { closed, .. } = entry {
            *closed = true;
        } else {
            break;  // Para ao encontrar entry que não é batch
        }
    }
}
```

**Propósito:**
- Marca os últimos ~4 batches abertos como `closed = true`
- Chamado ao chegar `Done` ou novo `UserMessage`
- Parar ao encontrar entry diferente (e.g., `AssistantText`) garante que não fecha batches de turns anteriores

---

### `flush_pending_narration_into_analysis(promote_last_to_text)`

```rust
fn flush_pending_narration_into_analysis(&mut self, promote_last_to_text: bool) {
    let raw = std::mem::take(&mut self.pending_narration);
    let mut lines: Vec<String> = raw
        .split('\n')
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())  // ← remove linhas vazias
        .collect();
    
    let final_text = if promote_last_to_text {
        lines.pop()  // ← última linha sai do batch
    } else {
        None
    };
    
    // Cada linha anterior → item do batch Analysis
    for line in lines {
        self.append_batch_item(BatchKind::Analysis, BatchItem {
            label: line,
            detail: String::new(),
            status: ToolItemStatus::Running,  // placeholder
        });
    }
    
    // Se `promote_last_to_text`, a última linha vira AssistantText
    if let Some(text) = final_text {
        self.close_all_open_batches();
        self.chat.push(ChatEntry::AssistantText(text));
    }
}
```

**Casos de uso:**
- `promote_last_to_text = false` → usado ao encontrar `ToolCall` no meio de uma cadeia
  - Todas as linhas do `pending_narration` viram items do batch `Analysis`
- `promote_last_to_text = true` → usado ao chegar `Done` dentro de uma cadeia
  - Primeiras N-1 linhas → batch `Analysis`
  - Última linha → `AssistantText` (resposta final)

---

### `flush_pending_narration_as_assistant_text()`

```rust
fn flush_pending_narration_as_assistant_text(&mut self) {
    let trimmed = self.pending_narration.trim();
    if trimmed.is_empty() {
        self.pending_narration.clear();
        return;
    }
    let text = trimmed.to_string();
    self.pending_narration.clear();
    self.close_all_open_batches();
    self.chat.push(ChatEntry::AssistantText(text));
    self.scroll_to_bottom();
}
```

**Propósito:**
- Transforma TODO o `pending_narration` em um único `AssistantText`
- Preserva quebras de linha internas (markdown, bullets)
- Chamado quando:
  - `!in_tool_chain` (modo resposta pura) ao encontrar `ToolCall`
  - Resposta "substantiva" (2+ linhas) ao encontrar `ToolCall` dentro de uma cadeia

---

## 📈 Decisão de Roteamento (TuiMsg::ToolCall)

```rust
TuiMsg::ToolCall { name, input } => {
    // Decidir destino do pending_narration:
    if !self.in_tool_chain 
        || self.pending_looks_like_substantial_response() 
    {
        // Modo A: Modo resposta ou resposta substantiva
        self.flush_pending_narration_as_assistant_text();
    } else {
        // Modo B: Pensamento curto dentro da cadeia
        self.flush_pending_narration_into_analysis(false);
    }
    
    self.in_tool_chain = true;
    // Criar novo item Tools
    self.append_batch_item(BatchKind::Tools, BatchItem { ... });
}
```

**Matriz de Decisão:**

| Estado | pending_narration | Ação | Resultado |
|--------|-------------------|------|-----------|
| `!in_tool_chain` | qualquer | → AssistantText | novo batch Tools |
| `in_tool_chain` + substantivo | 2+ linhas | → AssistantText | novo batch Tools |
| `in_tool_chain` + pensamento | 1 linha | → Analysis | novo batch Tools |

---

## 🧪 Testes Unitários

Todos os testes estão em `crates/elai-cli/src/tui.rs` (linhas ~5040+).

### Teste 1: Resposta pura (sem tools)

```rust
#[test]
fn text_chunks_buffer_until_done_then_become_assistant_text() {
    let mut app = make_app();
    app.apply_tui_msg(TuiMsg::TextChunk("Hello".to_string()));
    app.apply_tui_msg(TuiMsg::TextChunk(", World".to_string()));
    assert!(app.chat.is_empty());  // Nada no chat ainda
    assert_eq!(app.pending_narration, "Hello, World");
    
    app.apply_tui_msg(TuiMsg::Done);
    assert_eq!(app.chat.len(), 1);
    assert!(matches!(app.chat[0], ChatEntry::AssistantText(_)));
}
```

---

### Teste 2: Pensamento curto entre tools

```rust
#[test]
fn short_thought_between_tools_still_goes_to_analysis() {
    let mut app = make_app();
    app.apply_tui_msg(TuiMsg::ToolCall { name: "bash".into(), input: r#"..."# });
    app.apply_tui_msg(TuiMsg::ToolResult { ok: true });
    app.apply_tui_msg(TuiMsg::TextChunk("Vou checar agora.\n".into()));
    app.apply_tui_msg(TuiMsg::ToolCall { name: "bash".into(), input: r#"..."# });
    
    assert_eq!(app.chat.len(), 3);
    assert!(matches!(app.chat[1], ChatEntry::BatchEntry { 
        kind: BatchKind::Analysis, .. 
    }));
}
```

---

### Teste 3: Janela deslizante (BATCH_WINDOW)

```rust
#[test]
fn batch_window_drops_oldest_item_when_exceeded() {
    let mut app = make_app();
    for i in 0..7 {
        app.apply_tui_msg(TuiMsg::ToolCall { 
            name: format!("bash_{i}"), 
            input: r#"..."# 
        });
        app.apply_tui_msg(TuiMsg::ToolResult { ok: true });
    }
    
    // Apenas 5 items (bash_2 até bash_6)
    assert_eq!(items.len(), BATCH_WINDOW);
    assert_eq!(items[0].label, "bash_2");
    assert_eq!(items[4].label, "bash_6");
}
```

---

## 🎯 Resumo das Regras de Fluxo

### Resumo da Máquina de Estados

```
┌─────────────────────────────────────────────────────────────┐
│                     TURN STATE MACHINE                       │
└─────────────────────────────────────────────────────────────┘

[INITIAL: in_tool_chain=false, pending_narration=""]

          ┌─────────────────┐
          │  TextChunk...   │
          └────────┬────────┘
                   │
                   ├→ pending_narration += text
                   │
          ┌────────▼─────────┐
          │  ToolCall        │ ← Ponto de decisão!
          └────────┬─────────┘
                   │
        ┌──────────┴──────────┐
        │                     │
   !in_tool_chain?      in_tool_chain? 
   OR substantive?      & short thought?
        │                     │
        │◄────────────────────┤
        │       flush_pending_narration_as_assistant_text()
        │                     │
        ├→ close_all_open_batches()
        │                     │
        ├→ push(AssistantText)
        │                     │
        ├→ in_tool_chain = true
        │                     │
        └→ append_batch_item(Tools, ...)
               │
        ┌──────▼──────┐
        │ ToolResult  │
        └──────┬──────┘
               │
        ┌──────▼──────────────┐
        │  Mark last item     │
        │  as Ok/Err          │
        └──────┬──────────────┘
               │
      [Voltar para TextChunk ou Done]
               │
        ┌──────▼──────┐
        │   Done      │
        └──────┬──────┘
               │
         ┌─────▼─────────────────────────┐
         │ Flush pending_narration       │
         │ (modo escolhido por estado)   │
         └─────┬───────────────────────┘
               │
         ┌─────▼──────────────┐
         │  close_all_open_   │
         │  batches()         │
         └─────┬──────────────┘
               │
         ┌─────▼─────────────┐
         │  in_tool_chain=   │
         │  false            │
         └─────┬─────────────┘
               │
        [Turn termina, chat renderizado]
```

---

## 💡 Decisões de Design Explicadas

### 1. Por que `pending_narration` buffer?

**Problema:** TextChunks chegam fragmentados (rede, streaming).
**Solução:** Acumular em buffer até decisão ser possível (ao chegar ToolCall ou Done).
**Benefício:** Decisões acuradas sobre se texto é "pensamento" vs "resposta".

---

### 2. Por que `in_tool_chain` flag?

**Problema:** Distinguir "resposta do agente dentro de tool loop" de "resposta normal".
**Solução:** Flag que liga ao primeiro ToolCall, desliga ao chegar resposta substantiva.
**Benefício:** Roteamento de texto intermediário sem análise de conteúdo (velocidade).

---

### 3. Por que renderização em ordem invertida?

**Problema:** Tools antigas (base) poderiam ser "esquecidas" visualmente.
**Solução:** Renderizar mais recentes no topo, mantendo contexto.
**Benefício:** UX intuitiva — tool mais recente no olho.

---

### 4. Por que BATCH_WINDOW = 5?

**Problema:** Conversas longas com muitos tools podem sobrecarregar a TUI.
**Solução:** Limitar bloco a 5 items, descartar mais antigos.
**Benefício:** TUI responsiva, memória controlada, foco no presente.

---

## 🚀 Extensibilidade

### Adicionar um novo tipo de batch

```rust
// 1. Estender BatchKind
pub enum BatchKind {
    Tools,
    Analysis,
    NewType,  // ← novo
}

// 2. Renderização em chat_to_lines()
BatchKind::NewType => {
    result.push(Line::from(vec![
        Span::styled("  🆕 ".to_string(), ...),
        // ... render items
    ]));
}

// 3. Criar items conforme necessário
self.append_batch_item(BatchKind::NewType, item);
```

---

### Mudar heurística de "substantivo"

Se precisar mudar o que é considerado "resposta substantiva":

```rust
fn pending_looks_like_substantial_response(&self) -> bool {
    let trimmed = self.pending_narration.trim();
    
    // Nova lógica aqui
    // (por exemplo, usar comprimento em chars, contagem de palavras, etc)
    
    trimmed.len() > 100  // ← ao invés de linhas
}
```

---

## 🔍 Debugging Tips

### 1. Verificar estado de `pending_narration`

Adicionar log ao processar TextChunk:
```rust
eprintln!("📝 pending: {:?}", self.pending_narration);
```

### 2. Rastrear decisão de roteamento

No handler de `ToolCall`:
```rust
eprintln!("🔀 in_tool_chain={}, substantial={}", 
    self.in_tool_chain, 
    self.pending_looks_like_substantial_response()
);
```

### 3. Contar items em batches

```rust
for (i, entry) in app.chat.iter().enumerate() {
    if let ChatEntry::BatchEntry { kind, items, .. } = entry {
        eprintln!("Batch {}: {:?} ({} items)", i, kind, items.len());
    }
}
```

---

## 📚 Referências no Código

| Localização | Descrição |
|-------------|-----------|
| `crates/elai-cli/src/tui.rs:146-176` | Definições de `BatchKind`, `BatchItem`, `BatchEntry` |
| `crates/elai-cli/src/tui.rs:532-731` | Métodos `apply_tui_msg`, `append_batch_item`, `flush_*` |
| `crates/elai-cli/src/tui.rs:3551-3650` | Renderização em `chat_to_lines` |
| `crates/elai-cli/src/tui.rs:5031-5299` | Testes unitários |

---

## ✅ Conclusão

A implementação de batches é um exemplo de **design robusto** para UX em tempo real:

- ✅ **Simples de entender** (dois tipos: Tools e Analysis)
- ✅ **Responsiva** (rendering incremental, janela deslizante)
- ✅ **Bem testada** (9 testes cobrindo casos principais)
- ✅ **Extensível** (fácil adicionar novo `BatchKind`)
- ✅ **Semântica clara** (heurística de "substantivo" bem definida)

O sistema usa **state machines** e **buffering inteligente** para transformar stream fragmentado de eventos em visualização coerente e estruturada.
