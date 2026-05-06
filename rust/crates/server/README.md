# elai-server

Servidor HTTP (Rust + [Axum](https://github.com/tokio-rs/axum)) que expõe a stack do **elai-code**:
sessões de IA, workspace, Git, ferramentas, MCP, plugins, telemetria e cache — tudo via API REST
com streaming **SSE** e **WebSocket**.

> 📘 **Documentação interativa da API:** [`docs/site/index.html`](./docs/site/index.html)
> · Redoc: [`docs/site/redoc.html`](./docs/site/redoc.html)
> · Swagger UI: [`docs/site/swagger.html`](./docs/site/swagger.html)
> · OpenAPI 3.1: [`docs/site/openapi.yaml`](./docs/site/openapi.yaml)

---

## Sumário

- [Visão geral](#visão-geral)
- [Quick start](#quick-start)
- [Documentação da API](#documentação-da-api)
- [Autenticação](#autenticação)
- [Endpoints (resumo)](#endpoints-resumo)
- [Streaming (SSE & WebSocket)](#streaming-sse--websocket)
- [Configuração CLI](#configuração-cli)
- [Desenvolvimento](#desenvolvimento)

---

## Visão geral

`elai-server` é um binário standalone (`elai-server`) e uma biblioteca (`server`) reusável.
Por padrão escuta em `127.0.0.1:8456`, gera/persiste um token de autenticação em
`~/.elai/server-token` e protege todas as rotas (exceto `GET /v1/health`) com **Bearer token**.

Principais grupos de endpoints (`/v1`):

| Grupo          | Função                                                    |
| -------------- | --------------------------------------------------------- |
| `health`       | Healthcheck e versão                                      |
| `sessions`     | CRUD de sessões, mensagens, eventos SSE, permissões       |
| `workspace`    | `read`/`write`/`edit`/`glob`/`grep`/`tree`/`diff`         |
| `git`          | `status`, `diff`, `log`, `branches`, `commit`, PR, worktree |
| `config`       | Modelos, provedores, configuração global, budget e tema   |
| `commands`     | Slash-commands listáveis e executáveis por sessão         |
| `tools`        | Ferramentas LLM, allow/deny por sessão, rate-limit        |
| `tasks`        | Tarefas em segundo plano (run/cancel/output)              |
| `telemetry`    | Telemetria, sumário de uso, cache stats/clear             |
| `auth`         | Status, API keys, OAuth, import (Claude Code / Codex)     |
| `mcp`          | Servidores MCP, tools, resources, calls                   |
| `plugins`      | Plugins, skills, agents, hooks                            |
| `user-commands`| Slash-commands customizadas pelo usuário                  |

Total: **~70 endpoints** documentados em OpenAPI 3.1.

---

## Quick start

```bash
# 1. Subir o servidor (default: 127.0.0.1:8456, token em ~/.elai/server-token)
cargo run -p server --bin elai-server

# 2. Pegar o token
TOKEN=$(cat ~/.elai/server-token)

# 3. Healthcheck
curl http://127.0.0.1:8456/v1/health

# 4. Criar sessão
curl -X POST http://127.0.0.1:8456/v1/sessions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"cwd": "'"$PWD"'", "model": "claude-opus-4-7", "permission_mode": "workspace-write"}'

# 5. Stream SSE de eventos da sessão
curl -N -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8456/v1/sessions/<SESSION_ID>/events
```

---

## Documentação da API

### Servir localmente

```bash
./docs/serve.sh           # http://127.0.0.1:8080
./docs/serve.sh 9090      # porta customizada
```

Abre uma landing com 3 abas:

- **Redoc** — referência limpa em três colunas.
- **Swagger UI** — console interativo com *Try it out* (clique em **Authorize** e cole o `Bearer <token>`).
- **YAML** — especificação OpenAPI 3.1 crua.

### Abrir os arquivos diretamente

| Arquivo                                                  | Conteúdo                  |
| -------------------------------------------------------- | ------------------------- |
| [`docs/site/index.html`](./docs/site/index.html)         | Landing + navegação       |
| [`docs/site/redoc.html`](./docs/site/redoc.html)         | Redoc                     |
| [`docs/site/swagger.html`](./docs/site/swagger.html)     | Swagger UI                |
| [`docs/site/openapi.yaml`](./docs/site/openapi.yaml)     | OpenAPI 3.1 (fonte)       |
| [`docs/README.md`](./docs/README.md)                     | Como hospedar / regenerar |

> Para hospedar como GitHub Pages, aponte para `rust/crates/server/docs/site/`.

### Gerar SDKs / clientes

```bash
# TypeScript
npx openapi-typescript docs/site/openapi.yaml -o ./elai-server.d.ts

# Cliente Rust (reqwest)
openapi-generator-cli generate -i docs/site/openapi.yaml -g rust -o ./elai-client-rs
```

---

## Autenticação

Todas as rotas (exceto `GET /v1/health`) exigem:

```
Authorization: Bearer <token>
```

O token é:

- gerado automaticamente na primeira execução,
- persistido em `~/.elai/server-token` (ou no caminho passado via `--token-file`),
- validado por SHA-256 contra o token armazenado.

Para girar o token, basta apagar o arquivo e reiniciar o servidor.

---

## Endpoints (resumo)

> Lista compacta. Para schemas completos (request/response), consulte a documentação interativa.

### Health
- `GET /v1/health` *(público)* · `GET /v1/version`

### Sessions
- `POST /v1/sessions` · `GET /v1/sessions`
- `GET|DELETE|PATCH /v1/sessions/{id}`
- `POST /v1/sessions/{id}/messages` *(202 → turn_id)*
- `POST /v1/sessions/{id}/turns/{turn_id}/cancel`
- `GET /v1/sessions/{id}/events` *(SSE)*
- `GET /v1/sessions/{id}/cost`
- `GET /v1/sessions/{id}/permissions/pending`
- `POST /v1/permissions/{request_id}/decide`
- `GET /v1/sessions/{id}/permissions/ws` *(WebSocket)*
- `POST /v1/sessions/{id}/clone|compact|export|resume`

### Workspace
- `POST /v1/workspace/{session_id}/read|write|edit|glob|grep`
- `GET /v1/workspace/{session_id}/tree`
- `GET /v1/workspace/{session_id}/diff`

### Git
- `GET /v1/workspace/{session_id}/git/status|diff|log|branches`
- `POST /v1/workspace/{session_id}/git/commit`
- `POST /v1/workspace/{session_id}/git/branch/create`
- `POST /v1/workspace/{session_id}/git/worktree/create`
- `POST /v1/workspace/{session_id}/git/pr/create` *(requer `gh`)*

### Config
- `GET /v1/models` · `GET /v1/providers`
- `GET|PATCH /v1/config` · `GET /v1/config/sources`
- `POST /v1/providers/{id}/test`
- `GET|PATCH /v1/budget` · `GET|PATCH /v1/theme`

### Commands
- `GET /v1/commands`
- `POST /v1/sessions/{id}/commands/run|compact|export|resume`

### Tools
- `GET /v1/tools` · `GET /v1/tools/{name}`
- `POST /v1/sessions/{id}/tools/allow|deny`
- `GET /v1/tools/rate-limit`

### Tasks · Cache · Telemetry
- `GET /v1/tasks` · `GET /v1/tasks/{id}` · `GET /v1/tasks/{id}/output` · `POST /v1/tasks/{id}/cancel`
- `GET /v1/cache/stats` · `POST /v1/cache/clear`
- `GET /v1/telemetry` · `GET /v1/usage/summary`

### Auth
- `GET /v1/auth/status|methods`
- `POST /v1/auth/api-key` · `DELETE /v1/auth/api-key/{provider}`
- `POST /v1/auth/oauth/start|refresh` · `GET /v1/auth/oauth/callback`
- `POST /v1/auth/import/claude-code|codex`

### MCP
- `GET|POST /v1/mcp/servers`
- `PUT|DELETE /v1/mcp/servers/{name}`
- `POST /v1/mcp/servers/{name}/restart`
- `GET /v1/mcp/servers/{name}/tools|resources`
- `POST /v1/mcp/servers/{name}/tools/{tool}/call`

### Plugins · Skills · Agents · Hooks
- `GET /v1/plugins` · `POST|PUT|DELETE /v1/plugins/{name}`
- `GET /v1/skills` · `GET /v1/skills/validate`
- `GET /v1/agents` · `POST /v1/agents/{name}/run`
- `GET|PUT /v1/hooks`

### User Commands
- `GET|POST /v1/user-commands`
- `PUT|DELETE /v1/user-commands/{name}`

---

## Streaming (SSE & WebSocket)

### SSE — eventos da sessão

```bash
curl -N -H "Authorization: Bearer $TOKEN" \
  "http://127.0.0.1:8456/v1/sessions/$SID/events?since=0"
```

Eventos típicos: `snapshot`, `text_delta`, `thinking_delta`, `tool_use_started`,
`tool_use_input_delta`, `message_appended`, `tool_result`, `usage_delta`,
`turn_completed`, `turn_cancelled`, `turn_error`, `permission_request`.

### WebSocket — fluxo de permissões

```js
const ws = new WebSocket(`ws://127.0.0.1:8456/v1/sessions/${sid}/permissions/ws`, [], {
  headers: { Authorization: `Bearer ${token}` },
});
ws.onmessage = (msg) => {
  const evt = JSON.parse(msg.data); // { request_id, tool_name, input, required_mode }
  ws.send(JSON.stringify({ request_id: evt.request_id, outcome: "allow" }));
};
```

---

## Configuração CLI

```text
elai-server [OPTIONS]

OPTIONS:
      --listen <SOCKET>     Endereço de bind  [default: 127.0.0.1:8456]
      --cors-origin <STR>   CORS origin (informacional)
      --token-file <PATH>   Caminho do arquivo de token (default: ~/.elai/server-token)
  -h, --help                Imprime ajuda
  -V, --version             Imprime versão
```

Variável de ambiente útil: `RUST_LOG=info` (ou `debug`) para tracing.

---

## Desenvolvimento

```bash
# Build
cargo build -p server

# Run
cargo run -p server --bin elai-server

# Test
cargo test -p server
```

Estrutura interna relevante:

```
src/
├── lib.rs               # builder do Router (axum)
├── main.rs              # binário elai-server
├── auth.rs              # middleware Bearer + token utils
├── state.rs             # AppState compartilhado
├── streaming.rs         # helpers SSE
├── permission_bridge.rs # ponte de permissões → runtime
├── runtime_bridge.rs    # ponte runtime → eventos da sessão
├── db.rs                # persistência
└── routes/              # handlers por área (sessions, workspace, git, …)
```

A documentação OpenAPI vive em [`docs/site/openapi.yaml`](./docs/site/openapi.yaml) e é
versionada junto ao código — atualize-a sempre que adicionar/alterar endpoints.

---

## Licença

Proprietary. Veja a raiz do repositório para detalhes.
