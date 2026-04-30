# Changelog

Todas as mudanças notáveis deste projeto serão documentadas aqui.

O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/),
e o projeto adere a [Semantic Versioning](https://semver.org/lang/pt-BR/).

## [0.9.1] - 2026-04-29

Hotfix do build em CI cross-compile (musl) que quebrou em 0.9.0 ao
adicionar `native-tls`. Mantém todas as features do 0.9.0 + correção do
TLS sem dependência de `openssl-sys`.

### Corrigido

- **Build em `aarch64-unknown-linux-musl`**: 0.9.0 adicionou a feature
  `native-tls` ao `reqwest` na crate `tools` para resolver o problema
  de TLS handshake com Railway no macOS. Isso introduziu `openssl-sys`
  como dependência, que quebra o cross-compile pra musl no CI
  (`Could not find directory of OpenSSL installation`).
- Substituída a feature por `rustls-tls-native-roots`: continua usando
  rustls (puro Rust, sem deps C), mas agora lê os root certificates do
  **sistema** (Keychain no macOS, `/etc/ssl/certs` no Linux, cert store
  no Windows) em vez de `webpki-roots` embutido.
- Resolve a causa raiz do bug original (snapshot estática de roots
  desatualizada) sem trade-off no build cross-platform.
- Removida chamada `.use_native_tls()` em `dr_client()` — volta ao
  default do reqwest com a feature acima ativada.

### Notas

- Todas as features e correções do 0.9.0 permanecem ativas.
- Veja entrada [0.9.0] abaixo para a lista completa.

---

## [0.9.0] - 2026-04-29

Esta versão introduz a tool **DeepResearch** — pesquisa profunda na web com
streaming SSE — e uma série de melhorias de UX na TUI: blocos visuais para
tasks ao vivo, spinners variados por tipo de operação, janela rolante de
eventos e um modal dedicado de ativação.

### Adicionado

#### Tool `DeepResearch`

- Nova tool builtin `DeepResearch` que consome o serviço
  [`elai-deepresearch-rs`](https://github.com/nextlw/elai-deepresearch-rs)
  via HTTP streaming SSE (`POST /v1/chat/completions`).
- Streaming end-to-end: thinking, queries expandidas por persona, URLs
  visitadas e citações chegam em tempo real do agente remoto.
- Resultado retornado ao modelo inclui o **raciocínio completo** do agente
  de pesquisa (não apenas a síntese final), permitindo que Claude raciocine
  sobre o processo de busca.
- Parâmetros: `query`, `reasoning_effort` (low/medium/high),
  `language_code` (BCP-47, default `pt-BR`).
- Suporte a múltiplas variáveis de ambiente para configuração:
  `DEEP_RESEARCH_URL` ou `DEEP_RESEARCH_BASE_URL` (URL do serviço),
  `DEEP_RESEARCH_API_KEY` ou `VITE_DEEP_RESEARCH_SECRET` (Bearer token).

#### Slash command `/deepresearch` (alias `/dr`)

- `/deepresearch` sem argumento: exibe status atual (ativada ou não).
- `/deepresearch <api-key>`: ativa a tool em background — testa SSE
  end-to-end (POST autenticado + leitura da primeira chunk), salva no
  `.env` local e seta no processo para uso imediato sem restart.
- Quando não ativada, abre **modal dedicado** com input mascarado
  (`OverlayKind::DeepResearchKeyInput`): cole a key, navegue com setas,
  Enter para salvar e testar, Esc para cancelar.
- Mensagem de orientação no chat instrui o usuário a solicitar acesso
  por email caso ainda não tenha a key.

#### Visualização de tasks na TUI

- `ChatEntry::TaskProgress` agora renderiza como **bloco visual** estilo
  code fence:
  ```
  ╭─ ⠋ DeepResearch ─
  │ · 🔎 Query: AI agent pricing
  │ · 🌐 [3] anthropic.com
  │ · 💭 Raciocínio do agente em curso…
  ╰──────
  ```
- **Cor da borda** específica por tipo de task (mesmo padrão dos
  blocos de código por linguagem):
  - `DeepResearch` → azul ciano
  - `Verify` → verde
  - `Agent` → roxo
  - `Plugin` → amarelo
  - Outras → cor `info` do tema
- Estados visuais finalizados: `✓` verde (sucesso), `✗` vermelho (falha),
  `⊘` amarelo (cancelado).

#### Spinners variados por operação

- Para tasks multilinha (DeepResearch), o spinner do header **muda
  conforme o tipo da operação atual**, baseado em
  [cli-spinners](https://github.com/sindresorhus/cli-spinners) /
  [rattles](https://github.com/vyfor/rattles):

  | Operação | Spinner |
  |---|---|
  | `🔎 Query` | Setas `← ↖ ↑ ↗ → ↘ ↓ ↙` |
  | `🌐 URL` | Globo `🌍 🌎 🌏` |
  | `💭 Thinking` | Dots3 `⠋ ⠙ ⠚ ⠞ ⠖ ⠦ ⠴ ⠲ ⠳ ⠓` |
  | `🔍 Conectando` | Scan `⣼ ⣹ ⢻ ⠿ ⡟ ⣏ ⣧ ⡖` |
  | `⚡ Batch/parallel` | Triângulos `◢ ◣ ◤ ◥` |
  | `✍️ Compilando` | Arc `◜ ◠ ◝ ◞ ◡ ◟` |
  | `📡 Stream` | Crescimento `▁ ▃ ▄ ▅ ▆ ▇ █` |
  | Default | Dots `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏` |

#### Janela rolante de eventos

- Bloco de DeepResearch mantém **histórico acumulado** de eventos
  (URLs, queries, raciocínios), mas exibe apenas as **últimas 5 linhas
  visuais** — eventos antigos rolam para fora pelo topo conforme novos
  chegam pelo final.
- Eventos longos (raciocínio) sofrem wrap automático em até 4 linhas,
  com elipse `…` se ultrapassar.

#### Suporte a argumento inline no slash palette

- Digitar `/deepresearch <key>` no palette agora funciona: quando o
  filtro contém um espaço e a parte antes corresponde a um comando
  registrado, o palette executa com o argumento intacto em vez de
  ficar travado no filtro vazio.

### Modificado

#### Robustez de TLS

- Crate `tools` agora habilita as features `native-tls` e `json` no
  reqwest, com `use_native_tls()` no client builder. Resolve falhas
  intermitentes de TLS handshake com endpoints Railway no macOS quando
  rustls/webpki-roots não consegue validar a cadeia.

#### Validação real de auth na ativação

- A ativação do DeepResearch agora testa contra o endpoint **autenticado**
  (`POST /v1/chat/completions`) em vez do `/v1/models` público — o
  middleware do servidor retorna 401/403 quando a key é inválida, então
  agora detectamos keys erradas antes de salvar.

#### Tratamento de eventos SSE de erro

- Captura `type: "error"` (antes ignorado silenciosamente) e propaga
  como `Err` para o modelo.
- Quando `answer` vem vazia mas o agente concluiu, o resultado inclui
  uma mensagem clara orientando o modelo a usar o raciocínio acumulado
  e as URLs visitadas em vez de descartar o tool result.

#### Integração com `runtime::with_task_default`

- A pesquisa do DeepResearch roda dentro de
  `with_task_default(TaskType::LocalWorkflow, ...)`, registrando a task
  no `TaskRegistry` global e emitindo progresso via `TaskProgressReporter`.
  O `ChannelSink` da TUI recebe os updates e renderiza no bloco visual.

### Corrigido

- **URL hardcoded**: o default agora aponta para a URL pública correta
  (`https://elai-deepresearch-rs-production.up.railway.app`).
  `dr_base_url()` ignora valores sem scheme (ex: hostnames internos do
  Railway que não funcionam fora da rede privada).
- **Sobrescrita acidental do `.env`**: a ativação só persiste a key
  depois de validar que o auth funciona, evitando substituir a key
  correta por uma errada.
- **`stream: true` na ativação**: o teste de ativação usa exatamente o
  mesmo caminho do uso real (POST com SSE + leitura da primeira chunk),
  garantindo que se a ativação passa, o uso real também funciona.

### Bibliotecas / Dependências

- `reqwest` (crate `tools`): features ampliadas para `["blocking",
  "rustls-tls", "native-tls", "json"]`.

---

## [0.8.0] - Versão anterior

Bump de versão e melhorias iniciais de DeepResearch (modal de input,
handling de conexão).

## [0.7.9] - Versão anterior

Refactor de `dr_activate` — simplificação do health check e melhoria de
mensagens de erro.

## [0.7.8] - Versão anterior

Adição de ultrathink, captura de mouse na TUI, fila de mensagens
pendentes.
