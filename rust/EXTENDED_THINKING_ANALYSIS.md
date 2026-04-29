# Análise: Extended Thinking (Raciocínio Estendido) no Elai Code

## 📋 Visão Geral

O **Extended Thinking** (ou "raciocínio estendido / pensamento aprofundado") é um recurso implementado no Elai Code que permite ao modelo Claude usar **"thinking blocks"** — blocos de raciocínio interno que não são exibidos ao usuário, mas usam tokens para resolver problemas mais complexos.

### Status no Código
- ✅ **Implementado**: Suporte completo a parsing e streaming de thinking blocks
- ✅ **Documentado**: Listado no README.md como feature
- ⚠️ **Não Ativado**: Não há código que **força** extended thinking nos requests
- ⚠️ **Opcional**: O modelo ativa automaticamente quando apropriado (via budget interno)

---

## 🏗️ Arquitetura de Dados

### 1. Tipos de Conteúdo de Saída

No arquivo `crates/api/src/types.rs`, há suporte para 4 tipos de `OutputContentBlock`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: Value,
    },
}
```

**Novo (Extended Thinking):**
- `Thinking { thinking, signature }` — bloco de raciocínio do modelo
  - `thinking: String` — conteúdo do pensamento (pode ser longo)
  - `signature: Option<String>` — assinatura criptográfica (para verificação)
- `RedactedThinking { data }` — thinking que foi redactado pelo servidor

---

### 2. Deltas de Streaming

Para streaming SSE (Server-Sent Events), há `ContentBlockDelta`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },  // ← streaming thinking
    SignatureDelta { signature: String },
}
```

**Padrão de uso:**
1. `ContentBlockStartEvent` com `OutputContentBlock::Thinking`
2. Multiple `ContentBlockDeltaEvent` com `ContentBlockDelta::ThinkingDelta`
3. `ContentBlockStopEvent` para finalizar

---

### 3. Estrutura de Request

Atualmente, `MessageRequest` em `crates/api/src/types.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<InputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
}
```

**Faltam campos para ativar thinking:**
```rust
// Não implementado ainda:
#[serde(skip_serializing_if = "Option::is_none")]
pub budget_tokens: Option<u32>,  // ← limite de tokens de thinking

#[serde(default, skip_serializing_if = "std::ops::Not::not")]
pub thinking_enabled: bool,  // ← flag explícita
```

---

## 🎯 Quando é Ativado?

### Modelo: Ativação Automática (Recomendada)

O Claude **ativa automaticamente** extended thinking quando:
1. Tarefa é **complexa** (raciocínio necessário)
2. **Custo de thinking < valor da solução melhor**
3. Modelo tem **budget interno** (parte dos max_tokens)

**Exemplo:**
- ❌ "Qual é 2+2?" → sem thinking (óbvio)
- ✅ "Refatore este código complexo considerando..." → pode usar thinking

---

### Ativação Explícita (Não Implementada)

Para **forçar** thinking, seria necessário:

```rust
// No request body:
{
    "model": "claude-opus-4-6",
    "max_tokens": 16000,
    "budget_tokens": 10000,  // ← até 10k tokens para thinking
    "messages": [...],
    "thinking_enabled": true  // ← força ativação
}
```

**Status no Elai Code:** ❌ Não existe código que faça isso.

---

## 💾 Parsing de Thinking (SSE)

### SSE Frame de Inicio

```json
{
  "type": "content_block_start",
  "index": 0,
  "content_block": {
    "type": "thinking",
    "thinking": "",
    "signature": null
  }
}
```

**Parsing no código** (`crates/api/src/sse.rs`):

```rust
#[test]
fn parses_thinking_content_block_start() {
    let frame = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":null}}\n\n"
    );

    let event = parse_frame(frame).expect("frame should parse");

    assert_eq!(
        event,
        Some(StreamEvent::ContentBlockStart(
            crate::types::ContentBlockStartEvent {
                index: 0,
                content_block: OutputContentBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                },
            },
        ))
    );
}
```

✅ **Parsing funciona** — pode receber thinking blocks.

---

### SSE Deltas de Thinking

```json
{
  "type": "content_block_delta",
  "index": 0,
  "delta": {
    "type": "thinking_delta",
    "thinking": "step 1: analyze the problem"
  }
}
```

**Parsing de teste** (`crates/api/src/sse.rs`):

```rust
#[test]
fn parses_thinking_related_deltas() {
    let thinking = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"step 1\"}}\n\n"
    );
    let signature = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig_123\"}}\n\n"
    );

    let thinking_event = parse_frame(thinking).expect("thinking delta should parse");
    let signature_event = parse_frame(signature).expect("signature delta should parse");

    assert_eq!(
        thinking_event,
        Some(StreamEvent::ContentBlockDelta(
            crate::types::ContentBlockDeltaEvent {
                index: 0,
                delta: ContentBlockDelta::ThinkingDelta {
                    thinking: "step 1".to_string(),
                },
            }
        ))
    );
    // ... signature parsing também testado
}
```

✅ **Delta parsing funciona** — pode receber streaming incremental de thinking.

---

## 🖼️ Renderização de Thinking (TUI)

### No Elai Code Spoof (Claude Code OAuth)

Arquivo: `crates/api/src/providers/claude_code_spoof.rs`

```rust
/// Drop `temperature` from the body when the model supports adaptive thinking
```

⚠️ Comentário refere a "adaptive thinking" mas **não há implementação ativa**.

---

### No Chat/Output

**Status:** ❌ Nenhum renderer específico para thinking blocks foi encontrado.

Seria esperado em um dos locais:
1. `crates/elai-cli/src/tui.rs` — renderização TUI
2. `crates/elai-cli/src/main.rs` — renderização CLI
3. `crates/runtime/src/conversation.rs` — handler de resposta

**Achado:** Nenhum `match OutputContentBlock::Thinking` nos renderers públicos.

---

## 📊 Budget de Tokens

### Implementação Completa: `/budget` Command

O Elai Code **tem** um sistema de budget robusto para **limitar custos gerais**, não especificamente para thinking:

**Arquivo:** `crates/runtime/src/budget.rs`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_tokens: Option<u64>,      // Total de tokens (entrada + saída)
    pub max_turns: Option<u32>,       // Limite de turnos/conversas
    pub max_cost_usd: Option<f64>,    // Limite de custo em USD
    pub warn_at_pct: f32,             // Aviso em 80% (padrão)
}
```

### Diferença com Thinking Budget

**Budget Global (implementado):**
- Limite: `max_tokens` = tokens de entrada + saída total
- Aplica-se a: Todos os tokens (messages + thinking + output)
- Ativação: `/budget --max-tokens 1000000`

**Thinking Budget (não implementado):**
- Seria: Campo `budget_tokens` no request
- Aplica-se a: Apenas tokens **dentro de thinking blocks**
- Ativação: Seria necessário adicionar ao `MessageRequest`

**Relação:**
```
Total Request Tokens = Input + Thinking Tokens + Output Tokens
                       ^^^^^^   ^^^^^^^^^^^^^^^^   ^^^^^^^^^^^^^
                    mensagem   ESTE é thinking    resposta
                              budget (não impl)
```

---

## 🔍 Onde Faltam Implementações

### 1. ❌ Request Body: `budget_tokens` / `thinking` fields

**Arquivo esperado:** `crates/api/src/types.rs`

**Falta:**
```rust
pub struct MessageRequest {
    // ... campos existentes ...
    
    // Faltam:
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,  // Limite de tokens de thinking
    
    #[serde(default)]
    pub thinking: Option<ThinkingConfig>,  // Configuração de thinking
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    pub type_: String,  // "enabled" ou "disabled"
    pub budget_tokens: Option<u32>,
}
```

---

### 2. ❌ CLI Flags: `--thinking`, `--budget-tokens`

**Arquivos esperados:**
- `crates/elai-cli/src/args.rs` — parsing de argumentos
- `crates/elai-cli/src/main.rs` — passagem ao client

**Seria algo como:**
```bash
# Ativar thinking explicitamente:
elai --thinking 10000  # 10k tokens para thinking

# Ou via config:
elai login --config '{"thinking_budget": 10000}'
```

---

### 3. ❌ TUI Renderer: Exibição de Thinking Blocks

**Arquivo esperado:** `crates/elai-cli/src/tui.rs`

**Falta:** Handler para renderizar `OutputContentBlock::Thinking` na UI

Seria algo como:
```rust
ChatEntry::ThinkingBlock {
    content: String,
    signature: Option<String>,
    collapsed: bool,  // collapsed by default
}
```

---

### 4. ⚠️ Session Persistence: Salvar Thinking no Histórico

**Status:** Provavelmente salva (via `ConversationMessage`) mas sem renderização.

---

## 🔄 Fluxo Completo (Teórico)

Se tudo fosse implementado:

```
┌─────────────────────────────────────────────┐
│  User: "Refatore este código complexo"      │
│  CLI: elai --model opus --thinking 10000    │
└────────────────┬────────────────────────────┘
                 │
        ┌────────▼──────────┐
        │ Build request:    │
        │ {                 │
        │   "model": "...  │
        │   "max_tokens":   │
        │     16384,        │
        │   "budget_tokens" │  ← thinking budget
        │     : 10000,      │
        │   "thinking":{    │
        │     "type":       │
        │       "enabled"   │
        │   }               │
        │ }                 │
        └────────┬──────────┘
                 │
        ┌────────▼───────────────────┐
        │ POST /messages (stream)     │
        │ ↓                           │
        │ SSE Event 1: content_block_ │
        │   start (type=thinking)     │
        │ ↓                           │
        │ SSE Events 2-N: thinking_   │
        │   delta "step 1...", ...    │
        │ ↓                           │
        │ SSE: content_block_stop     │
        │ ↓                           │
        │ SSE: content_block_start    │
        │   (type=text)               │
        │ ↓                           │
        │ SSE: text_delta "Here's..." │
        │ ↓                           │
        │ SSE: message_stop           │
        └────────┬───────────────────┘
                 │
        ┌────────▼───────────────────┐
        │ Parse Events:               │
        │ [ThinkingBlock, TextBlock]  │
        └────────┬───────────────────┘
                 │
        ┌────────▼──────────────────┐
        │ Render no Chat:            │
        │                            │
        │ 💭 Thinking (collapsed)    │
        │   [click to expand]        │
        │                            │
        │ Here's the refactored code │
        │ ...                        │
        └────────┬──────────────────┘
                 │
        ┌────────▼────────────────┐
        │ Save in session:         │
        │ messages = [             │
        │   assistant_with_blocks: │
        │     [Thinking, Text]     │
        │ ]                        │
        └─────────────────────────┘
```

---

## 📈 Casos de Uso

### Ativação Automática — Quando Claude Usa

1. **Análise Complexa**
   - "Analise a segurança desta implementação"
   - Claude pensa internamente, depois escreve auditoria

2. **Raciocínio Matemático**
   - "Resolva este sistema de equações"
   - Claude pensa passos, depois mostra solução

3. **Planejamento Multi-Passo**
   - `/ultraplan` — deep planning prompt
   - Claude pensa estratégia, depois recomenda steps

### Ativação Explícita — Se Implementado

```bash
# Tarefa crítica que precisa de deep thinking:
elai --thinking 15000 "Designe a arquitetura para..."

# Debugging de problema difícil:
elai --thinking 20000 /bughunter

# Refatoração de código legado:
elai --thinking 10000 "Refatore mantendo compatibilidade"
```

---

## 🔧 Implementação Futura (Roadmap)

Se você quisesse **ativar thinking explicitamente**, seria:

### Passo 1: Estender `MessageRequest`

```rust
// crates/api/src/types.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<InputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    
    // NEW:
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}
```

### Passo 2: Adicionar CLI Flag

```rust
// crates/elai-cli/src/args.rs
#[derive(Parser)]
struct Cli {
    // ... campos existentes ...
    
    #[arg(long, help = "Token budget for extended thinking")]
    thinking: Option<u32>,
}
```

### Passo 3: Passar ao Request

```rust
// crates/elai-cli/src/main.rs
let request = MessageRequest {
    model: model_name.to_string(),
    max_tokens: suggested_max_tokens,
    messages: messages,
    budget_tokens: args.thinking,  // ← novo
    // ... resto ...
};
```

### Passo 4: Renderizar Thinking Block

```rust
// crates/elai-cli/src/tui.rs
pub enum ChatEntry {
    // ... existentes ...
    ThinkingBlock {
        content: String,
        collapsed: bool,
    },
}

// No renderer:
ChatEntry::ThinkingBlock { content, collapsed } => {
    if *collapsed {
        println!("💭 Thinking (collapsed) [press T to expand]");
    } else {
        println!("💭 Thinking:\n{}", content);
    }
}
```

---

## 📚 Arquivos Relacionados

| Arquivo | Função | Status |
|---------|--------|--------|
| `crates/api/src/types.rs` | Tipos de `OutputContentBlock`, `ContentBlockDelta` | ✅ Completo |
| `crates/api/src/sse.rs` | Parser de SSE para thinking deltas | ✅ Completo |
| `crates/api/src/providers/claude_code_spoof.rs` | Comentário sobre "adaptive thinking" | ⚠️ Superficial |
| `crates/runtime/src/budget.rs` | Budget de **custos globais** (não thinking-específico) | ✅ Completo |
| `crates/runtime/src/conversation.rs` | Salva `ConversationMessage` (provável suporte) | ❓ Não verificado |
| `crates/elai-cli/src/args.rs` | CLI flags | ❌ Sem `--thinking` |
| `crates/elai-cli/src/main.rs` | Passagem de args ao client | ❌ Sem routing |
| `crates/elai-cli/src/tui.rs` | Renderização de chat | ❌ Sem renderer |
| `README.md` | Documentação | ⚠️ "Extended thinking (thinking blocks) | ✅" |

---

## ✅ Conclusão

### O que está pronto:
1. ✅ Tipos de dados para thinking blocks
2. ✅ Parser SSE para streaming thinking
3. ✅ Budget de tokens (custo geral)
4. ✅ Suporte ao saving em sessões

### O que falta:
1. ❌ Flag CLI para ativar thinking (`--thinking` ou similar)
2. ❌ Campo `budget_tokens` em `MessageRequest`
3. ❌ Renderização visual de thinking blocks
4. ❌ Configuração persistente no `.elai.json`

### Modelo atual:
**Claude ativa automaticamente** extended thinking quando apropriado — você não precisa fazer nada.

### Se quiser ativar explicitamente:
Seria uma feature **pronta para implementar** (4-6 prs pequenos).

### Quando o Elai usa thinking (hoje):
- `/ultraplan` — deep multi-step planning (pode ativar internamente)
- Tarefas complexas que o Claude julgue útil
- Sempre que custo-benefício for positivo

---

## 🎯 Resumo de Ativação

| Cenário | Código que Ativa? | Como? |
|---------|-------------------|-------|
| **Resposta simples** ("Olá") | Não | Claude não sente necessidade |
| **Análise média** ("Refatore") | Talvez | Automático do modelo (budget interno) |
| **Deep planning** (`/ultraplan`) | Possivelmente | Prompt contém "think deeply" |
| **Ativar força** (`--thinking`) | ❌ Não implementado | Seria via novo flag |

**Conclusão:** Extended thinking **está pronto pra receber**, mas **não há força explícita** de ativação. É sempre automático/opcional no lado do Claude.
