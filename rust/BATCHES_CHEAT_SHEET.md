# Guia Rápido de Batches — Cheat Sheet

## 🎯 O que é um Batch?

Um **batch** é um agrupamento visual de eventos relacionados na TUI. Existem **dois tipos:**

```
┌─────────────────────────┐
│  ⚙ Tools (3)            │  ← Batch de TOOLS
│    ✓ bash · ls          │     (chamadas de ferramentas)
│    ✓ read_file · path   │
│    ⠋ write_file · ...   │
└─────────────────────────┘

┌─────────────────────────┐
│  ⚙ Analise (2)          │  ← Batch de ANALYSIS
│    💬 Encontrado!       │     (pensamentos do agente)
│    💬 Vou processar.    │
└─────────────────────────┘
```

---

## 🔑 Estados Críticos

```rust
struct UiApp {
    pending_narration: String,  // ← Buffer de texto não-renderizado
    in_tool_chain: bool,        // ← true = dentro de tools
    chat: Vec<ChatEntry>,
}

enum ChatEntry {
    TextChunk → acumula em pending_narration
    ToolCall  → decide destino de pending (→ AssistantText ou Analysis)
    ToolResult → marca status (Ok/Err)
    Done      → libera pending_narration final, fecha batches
}
```

---

## 💡 Decisão Crucial: "Pensamento" ou "Resposta"?

**Quando chega um `ToolCall`:**

```
Checar: pending_looks_like_substantial_response() ?

┌─────────────────────────────┐
│ É SUBSTANTIVO?              │
│ (2+ linhas OU \n\n)         │
└──────────┬──────────────────┘
           │
    ┌──────▼──────────┐
    │ SIM: Grande     │ NO: Curto
    │ resposta        │ pensamento
    │                 │
    │ → AssistantText │ → Analysis Batch
    │ (novo turn)     │ (continua chain)
    └─────────────────┘
```

**Exemplos:**

| Texto | Tipo | Ação |
|-------|------|------|
| `"Vou checar."` | Curto | → Analysis batch |
| `"Achei 3 pontos:\n- A\n- B"` | Substantivo | → AssistantText |
| `"Para responder:\n\nEu penso..."` | Substantivo | → AssistantText |

---

## 🔄 Ciclo de Vida de um Batch

```
1. Primeiro ToolCall
   └─→ append_batch_item(Tools, ...)
       └─→ Novo BatchEntry criado (closed: false)

2. Mais ToolCalls (mesmo tipo)
   └─→ append_batch_item(Tools, ...)
       └─→ Anexa ao BatchEntry aberto
           └─→ Se >5 items: remove o mais antigo

3. TextChunk intermediário (curto)
   └─→ pending_narration += text
       └─→ Flushed no próximo ToolCall
           └─→ → Analysis batch NOVO
               └─→ Break no agrupamento de Tools

4. Done (fim do turn)
   └─→ close_all_open_batches()
       └─→ Todos BatchEntry.closed = true
           └─→ Próximo item abre batch novo
```

---

## 📊 Máquina de Estados Simplificada

```
         [TURN START]
              ↓
         TextChunk
         accumulated
         in pending_narration
              ↓
         ToolCall?
         /        \
       YES         NO → Done → TURN END
        │                       ↓
    Substantial?              Flush all
    /          \              pending as
   YES         NO             AssistantText
    │           │
    │      Analysis
    │      Batch
    │           │
    └───┬───────┘
        ↓
    New Tools Batch
    (or continue if NO)
        ↓
    ToolResult
    (mark Ok/Err)
        ↓
   [loop back]
```

---

## 🧩 Estrutura de Dados (Ultra-Resumida)

```rust
// Tipo de agrupamento
pub enum BatchKind { Tools, Analysis }

// Item dentro do batch
pub struct BatchItem {
    label: String,      // Nome da tool / frase narrativa
    detail: String,     // Input resumido / vazio
    status: Status,     // Running | Ok | Err (só para Tools)
}

// Bloco visual
pub enum ChatEntry {
    BatchEntry {
        kind: BatchKind,
        items: Vec<BatchItem>,  // Até 5 items (sliding window)
        closed: bool,           // Aberto (vivo) ou fechado
    }
}
```

---

## 🎨 Renderização (Código)

```rust
// Loop em items INVERTIDO (mais recente no topo)
for item in items.iter().rev() {
    match kind {
        Tools => {
            // Mostrar: ✓/✗/⠋ nome · input_resumido
        }
        Analysis => {
            // Mostrar: 💬 frase_narrativa
        }
    }
}
```

**Resultado visual:**
```
⚙ Tools (3)
  ✓ write_file  ← mais recente (topo)
  ✓ read_file
  ⠋ bash        ← mais antigo (base)
```

---

## 🔄 Métodos-Chave (Resumo)

### `append_batch_item(kind, item)`
```
Se último entry é BatchEntry aberto do MESMO kind:
  → Anexa item (e remove o mais antigo se >5)
Senão:
  → Cria novo BatchEntry
```

### `close_all_open_batches()`
```
Marca os últimos ~4 BatchEntries como closed: true
(Para ao encontrar entry que não seja batch)
```

### `flush_pending_narration_into_analysis(promote_last: bool)`
```
Pega pending_narration e cria items do batch Analysis
Se promote_last = true:
  → última linha vira AssistantText (resposta final)
Senão:
  → todas linhas viram items Analysis (pensamento)
```

### `flush_pending_narration_as_assistant_text()`
```
Converte TODO pending_narration em um único AssistantText
Preserva estrutura (markdown, bullets)
Fecha todos batches abertos antes
```

---

## 🎯 Heurística em Uma Linha

```rust
fn is_substantial(text: &str) -> bool {
    text.contains("\n\n") ||           // Parágrafo
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .count() > 1                    // 2+ linhas
}
```

---

## 📈 Diagrama de Decisão

```
         Chega ToolCall
              ↓
      ┌───────┴──────────┐
      │                  │
    in_tool_chain?    in_tool_chain?
    = false           = true
      │                  │
      │          substantial_response?
      │          /              \
      │         YES             NO
      │         │               │
      └─────┬───┘               │
            │                   │
      flush_pending             │
      as FULL                   │
      AssistantText             │
            │                   │
            │          flush_pending
            │          into_analysis
            │          (short items)
            │                   │
            ├───────────┬───────┘
                        ↓
                  mark in_tool_chain=true
                        ↓
                  append_batch_item(Tools)
                        ↓
                  New/Continue batch
```

---

## 🧪 3 Testes Essenciais

### Teste 1: Resposta Pura
```rust
TextChunk("Hello") + TextChunk(" World") + Done
→ Chat: [AssistantText("Hello World")]
```

### Teste 2: Cadeia de Tools
```rust
ToolCall + ToolResult(ok) +
ToolCall + ToolResult(ok) +
ToolCall + ToolResult(ok) +
Done
→ Chat: [BatchEntry { Tools, 3 items, Ok }]
```

### Teste 3: Pensamento Curto
```rust
ToolCall + ToolResult(ok) +
TextChunk("Vou.\n") +
ToolCall
→ Chat: [
    BatchEntry { Tools, 1 item },
    BatchEntry { Analysis, 1 item "Vou." },
    BatchEntry { Tools, 1 item }
  ]
```

---

## ⚡ Performance Notes

- **BATCH_WINDOW = 5:** Conversa longa → mantém apenas 5 tools visíveis
- **Renderização invertida:** O(n) onde n=5 max
- **pending_narration buffer:** Evita parsing contínuo de fragments
- **in_tool_chain flag:** Decisão O(1) ao invés de análise de strings

---

## 🚀 Como Estender

### Adicionar novo BatchKind

```rust
// 1. Enum
pub enum BatchKind {
    Tools,
    Analysis,
    MyNewType,  // ← ADD
}

// 2. Renderização (chat_to_lines)
MyNewType => {
    result.push(Line::from(vec![
        Span::styled("  🆕 MyType", ...),
        // render items
    ]));
}

// 3. Usar
append_batch_item(BatchKind::MyNewType, item)
```

---

## 🐛 Debugging Rápido

```bash
# Ver estado de pending_narration
eprintln!("📝 pending: {}", app.pending_narration);

# Ver decisão de roteamento
eprintln!("🔀 in_tool_chain={}, substantial={}",
    app.in_tool_chain,
    app.pending_looks_like_substantial_response()
);

# Contar items por batch
for (i, e) in app.chat.iter().enumerate() {
    if let ChatEntry::BatchEntry { kind, items, .. } = e {
        eprintln!("{}: {:?} ({} items)", i, kind, items.len());
    }
}
```

---

## 📚 Links Rápidos

| O quê | Onde |
|-------|------|
| Definições | `tui.rs:146-176` |
| apply_tui_msg | `tui.rs:669-755` |
| append_batch_item | `tui.rs:574-591` |
| Renderização | `tui.rs:3551-3650` |
| Testes | `tui.rs:5031-5299` |

---

## ✅ Quick Checklist

- [ ] Entendi os 2 tipos de batch (Tools vs Analysis)
- [ ] Sei o que `pending_narration` faz
- [ ] Conheço o significado de `in_tool_chain`
- [ ] Posso traçar um ToolCall até o resultado visual
- [ ] Consigo estender com novo BatchKind
- [ ] Posso debugar um problema de renderização

---

**Leia `ANALISE_BATCHES.md` para detalhes completos!** 📖
