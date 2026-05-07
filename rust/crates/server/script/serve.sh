#!/usr/bin/env bash
# Dev bootstrap: sobe infraestrutura (compose) + migrations + elai-server + serve docs.
#
# Uso:
#   ./serve.sh              # tudo com defaults
#   ./serve.sh 9090         # docs na porta customizada
#   SKIP_INFRA=1 ./serve.sh  # pula compose (infra já rodando)
#   SKIP_SERVER=1 ./serve.sh  # só sobe infra, não o server
#   SKIP_DOCS=1 ./serve.sh   # sobe infra+server sem servir docs (fica em foreground)

set -uo pipefail

# enable pipefail only for critical sections, off by default for dev resilience
pipefail_on()  { set -o pipefail; }
pipefail_off() { set +o pipefail; }

# ── defaults ──────────────────────────────────────────────────────────────────
# DOCS_PORT  = porta do servidor estático de docs (primeiro arg, separado do API server)
# SERVER_PORT = porta do elai-server (deve bater com o proxy do Vite — vite.config.ts aponta pra 8080)
DOCS_PORT="${1:-9090}"
SERVER_PORT="${SERVER_PORT:-8080}"
SERVER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"       # rust/crates/server/
COMPOSE_FILE="${SERVER_DIR}/docker-compose.yml"
ENV_FILE="${SERVER_DIR}/.env"
RUST_DIR="$(cd "$SERVER_DIR/../.." && pwd)"                            # rust/
# garante trailing slash
[[ "$RUST_DIR" != */ ]] && RUST_DIR="${RUST_DIR}/"

# credenciais padrão (dev)
export PG_DB="${PG_DB:-nexa_dev}"
export PG_USER="${PG_USER:-nexa}"
export PG_PASSWORD="${PG_PASSWORD:-nexa123}"
export PG_PORT="${PG_PORT:-5433}"
export QDRANT_PORT="${QDRANT_PORT:-6333}"
export QDRANT_GRPC_PORT="${QDRANT_GRPC_PORT:-6334}"
export REDIS_PORT="${REDIS_PORT:-6379}"

# frontend
export FRONT_DIR="${FRONT_DIR:-}"
export FRONT_PORT="${FRONT_PORT:-5174}"

# ── cores ─────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GRN='\033[0;32m'; YEL='\033[1;33m'; CYA='\033[0;36m'; BLD='\033[1m'
MAG='\033[0;35m'; BLU='\033[0;34m'; WHT='\033[0;37m'
RES='\033[0m'
info()  { echo -e "${CYA}[INFO]${RES} $*"; }
ok()    { echo -e "${GRN}[ OK ]${RES} $*"; }
warn()  { echo -e "${YEL}[WARN]${RES} $*"; }
die()   { echo -e "${RED}[FAIL]${RES} $*" >&2; exit 1; }

# cores por processo
C_SERVER="${GRN}"; C_FRONT="${MAG}"; C_DOCS="${BLU}"

# tag_stream COLOR LABEL — lê stdin e prefixa cada linha com [LABEL] colorido
tag_stream() {
  local color="$1" label="$2"
  while IFS= read -r line; do
    printf "${color}%-20s${RES} %s\n" "$label" "$line"
  done
}

# array global de PIDs em foreground para wait/cleanup
declare -a ALL_PIDS=()

# ── helpers ──────────────────────────────────────────────────────────────────
docker_running()  { docker info > /dev/null 2>&1; }

start_docker_macos() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    warn "start Docker automático só suportado no macOS"
    return 1
  fi
  info "iniciando Docker..."
  open -a Docker 2>/dev/null || {
    # tenta pelo launchctl (mais antigo)
    launchctl start Boot第Start/docker 2>/dev/null || \
    warn "não consegui iniciar o Docker automaticamente — abra manualmente"
    return 1
  }
  info "aguardando Docker iniciar..."
  for i in $(seq 1 30); do
    if docker info > /dev/null 2>&1; then
      ok "Docker operacional (${i}s)"
      return 0
    fi
    sleep 1
  done
  die "Docker não iniciou após 30s. Abra o Docker Desktop manualmente."
}

wait_docker() {
  info "aguardando Docker..."
  for i in $(seq 1 60); do
    if docker info > /dev/null 2>&1; then
      ok "Docker pronto (${i}s)"
      return 0
    fi
    sleep 1
  done
  die "Docker não ficou disponível após 60s."
}

ask_start_docker() {
  # detecta terminal interativo: read -t 0 retorna 0 se stdin tem caracteres disponíveis
  if ! read -t 0 2>/dev/null; then
    warn "stdin não interativo — abrindo Docker automaticamente..."
    start_docker_macos
    return
  fi

  local choice
  echo -e "${YEL}[?] Docker não está rodando.${RES}"
  echo ""
  echo -e "   1) ${CYA}Iniciar pra mim${RES}  (abre Docker.app e espera ficar pronto)"
  echo -e "   2) ${GRN}Já vou abrir na mão${RES} → aguarda até 60s"
  echo -e "   3) ${RED}Sair${RES}"
  echo ""
  echo -n "   [2]: "; read choice

  case "${choice:-2}" in
    1) start_docker_macos ;;
    2) wait_docker ;;
    3) die "abortado" ;;
    *) warn "padrão: aguardando..."; wait_docker ;;
  esac
}
compose_cmd() {
  if docker compose version > /dev/null 2>&1; then
    docker compose "$@"
  elif docker-compose --version > /dev/null 2>&1; then
    docker-compose "$@"
  else
    return 1
  fi
}

compose_running() {
  compose_cmd -f "$COMPOSE_FILE" ps -q postgres 2>/dev/null | grep -q .
}

wait_service() {
  local host="$1" port="$2" name="$3" max_wait="${4:-30}"
  info "aguardando ${name} em ${host}:${port}..."
  for i in $(seq 1 "$max_wait"); do
    if nc -z "$host" "$port" 2>/dev/null; then
      ok "${name} operacional (${i}s)"
      return 0
    fi
    sleep 1
  done
  die "${name} não ficou pronto em ${max_wait}s. Verifique: docker compose -f \"$COMPOSE_FILE\" logs"
}

ask_front_path() {
  # FRONT_DIR já vem do .env → pergunta só se quer ativar ou pular
  if [[ -n "${FRONT_DIR:-}" ]]; then
    if [[ ! -d "$FRONT_DIR" ]]; then
      warn "FRONT_DIR='$FRONT_DIR' não encontrado — pulando frontend"
      FRONT_DIR=""
      return
    fi

    if ! read -t 0 2>/dev/null; then
      info "frontend: $FRONT_DIR (auto, stdin não interativo)"
      return
    fi

    echo ""
    echo -e "  ${YEL}[?] Frontend detectado no .env:${RES}"
    echo -e "       ${CYA}${FRONT_DIR}${RES}"
    echo ""
    echo -n "       Subir frontend? [S/n] "
    read answer
    answer=$(echo "${answer:-s}" | tr '[:upper:]' '[:lower:]')

    case "$answer" in
      n|nao|não) info "frontend pulado"; FRONT_DIR=""; return ;;
      *) ok "frontend: $FRONT_DIR" ; return ;;
    esac
  fi

  # sem FRONT_DIR → pergunta caminho
  echo ""
  echo -e "${YEL}[?] Caminho do frontend${RES}"
  echo "   (vite / next / react — deixa vazio para pular)"
  echo ""
  echo -n "   path: "; read answer

  answer=$(echo "$answer" | xargs)
  if [[ -z "$answer" ]]; then
    info "frontend pulado"
    return
  fi

  if [[ ! -d "$answer" ]]; then
    warn "diretorio '$answer' não existe — tentando criar..."
    mkdir -p "$answer" 2>/dev/null || {
      warn "não consegui criar '$answer' — pulando frontend"
      return
    }
  fi

  FRONT_DIR="$answer"
  export FRONT_DIR
  ok "frontend: $FRONT_DIR"
}

# ── sanity checks ─────────────────────────────────────────────────────────────
if [[ ! -d "$SERVER_DIR/src" ]]; then
  die "Não encontrei o código do server em: $SERVER_DIR"
fi
if [[ ! -f "$COMPOSE_FILE" ]]; then
  die "docker-compose.yml não encontrado em: $COMPOSE_FILE"
fi

# ── load .env existente (não sobrescreve vars já exportadas) ─────────────────
if [[ -f "$ENV_FILE" ]]; then
  info "carregando $ENV_FILE"
  while IFS='=' read -r key val; do
    [[ "$key" =~ ^[[:space:]]*# ]] && continue
    [[ -z "$key" ]] && continue
    # não sobrescreve se já está no ambiente (permite override por CLI)
    export "${key}=${val}" 2>/dev/null || true
  done < "$ENV_FILE"
fi

# ── garantir DATABASE_URL exportado (fallback) ───────────────────────────────
if [[ -z "${DATABASE_URL:-}" ]]; then
  export DATABASE_URL="postgres://${PG_USER}:${PG_PASSWORD}@localhost:${PG_PORT}/${PG_DB}"
fi

# ══════════════════════════════════════════════════════════════════════════════
# 0. FRONTEND PATH
# ══════════════════════════════════════════════════════════════════════════════
ask_front_path

# ══════════════════════════════════════════════════════════════════════════════
# 1. INFRAESTRUTURA
# ══════════════════════════════════════════════════════════════════════════════
if [[ "${SKIP_INFRA:-}" != "1" ]]; then
  if ! docker_running; then
    if [[ -t 0 ]]; then
      ask_start_docker
    else
      warn "stdin não interativo — abrindo Docker automaticamente..."
      start_docker_macos
    fi
  fi

  if compose_running; then
    ok "infraestrutura já está rodando"
  else
    info "subindo compose (postgres + qdrant + redis)..."
    # passa vars como env pro compose
    PG_DB="$PG_DB" PG_USER="$PG_USER" PG_PASSWORD="$PG_PASSWORD" \
    PG_PORT="$PG_PORT" QDRANT_PORT="$QDRANT_PORT" \
    QDRANT_GRPC_PORT="$QDRANT_GRPC_PORT" REDIS_PORT="$REDIS_PORT" \
    compose_cmd -f "$COMPOSE_FILE" up -d

    ok "containers subidos"
  fi

  # health checks
  wait_service localhost "$PG_PORT"    "PostgreSQL"
  wait_service localhost "$QDRANT_PORT" "Qdrant"   45
  wait_service localhost "$REDIS_PORT"  "Redis"
  ok "infraestrutura completa"
else
  info "SKIP_INFRA=1 — pulando compose (usando serviços externos)"
fi

# ══════════════════════════════════════════════════════════════════════════════
# 2. GERAR .env (para bind mount / referência)
# ══════════════════════════════════════════════════════════════════════════════
info "garantindo .env com defaults corretos..."

write_env() {
  local f="$ENV_FILE"
  cat > "$f" <<EOF
# ── auto-gerado pelo serve.sh (pode editar manualmente) ──
# só é recriado se não existir ou se vars de ambiente divergirem

DATABASE_URL=postgres://${PG_USER}:${PG_PASSWORD}@localhost:${PG_PORT}/${PG_DB}
XAI_API_KEY=${XAI_API_KEY:-}
ELAI_MODEL=${ELAI_MODEL:-go:kimi-k2.6}
XAI_BASE_URL=${XAI_BASE_URL:-https://api.x.ai/v1}
PORT=${SERVER_PORT}

# Clerk (obrigatório para produção — pode deixar vazio em dev local)
CLERK_JWKS_URL=${CLERK_JWKS_URL:-https://your-clerk-domain.clerk.accounts.dev/.well-known/jwks.json}
CLERK_WEBHOOK_SECRET=${CLERK_WEBHOOK_SECRET:-}
CLERK_SECRET_KEY=${CLERK_SECRET_KEY:-}

# Qdrant
QDRANT_URL=http://localhost:${QDRANT_PORT}

# Redis
REDIS_URL=redis://localhost:${REDIS_PORT}/0

# Frontend
FRONT_DIR=${FRONT_DIR:-}
FRONT_PORT=${FRONT_PORT:-5173}

# Docs (porta do servidor http local)
DOCS_PORT=${DOCS_PORT}
EOF
  ok ".env gerado em $f"
  echo "  → DATABASE_URL=${DATABASE_URL}"
  echo "  → QDRANT_URL=http://localhost:${QDRANT_PORT}"
  echo "  → REDIS_URL=redis://localhost:${REDIS_PORT}/0"
}

if [[ -f "$ENV_FILE" ]]; then
  # já existe — pergunta se quer regenerar
  if [[ "${FORCE_ENV:-}" == "1" ]]; then
    write_env
  else
    info ".env já existe — mantém atual. FORCE_ENV=1 para regerar."
  fi
else
  write_env
fi

# ══════════════════════════════════════════════════════════════════════════════
# 3. MIGRATIONS
# ══════════════════════════════════════════════════════════════════════════════
info "verificando banco de dados..."
export PGPASSWORD="$PG_PASSWORD"
CREATEDB_SQL="SELECT 1 FROM pg_database WHERE datname='${PG_DB}'"
if ! PGPASSWORD="$PG_PASSWORD" psql -h localhost -p "$PG_PORT" -U "$PG_USER" -d postgres -tAc "$CREATEDB_SQL" 2>/dev/null | grep -q 1; then
  info "criando banco '$PG_DB'..."
  # usa psql com -w (sem password prompt) ou cria via docker
  PGPASSWORD="$PG_PASSWORD" psql -h localhost -p "$PG_PORT" -U "$PG_USER" -d postgres -c "CREATE DATABASE \"${PG_DB}\";" -w 2>/dev/null \
    || PGPASSWORD="$PG_PASSWORD" psql -h localhost -p "$PG_PORT" -U "$PG_USER" -d postgres -c "CREATE DATABASE \"${PG_DB}\";" <<< "" 2>/dev/null \
    || info "banco pode já existir ou falhou — continuando"
  ok "banco '$PG_DB' pronto"
else
  ok "banco '$PG_DB' já existe"
fi

info "aplicando migrations..."
MIGRATIONS_DIR="${SERVER_DIR}/migrations"
if [[ -d "$MIGRATIONS_DIR" ]]; then
  shopt -s nullglob
  migrations=("$MIGRATIONS_DIR"/*.sql)
  shopt -u nullglob
  if [[ ${#migrations[@]} -gt 0 ]]; then
    for mig in "${migrations[@]}"; do
      info "  $(basename "$mig")..."
      PGPASSWORD="$PG_PASSWORD" psql -h localhost -p "$PG_PORT" -U "$PG_USER" -d "$PG_DB" -f "$mig" 2>/dev/null \
        || warn "  $(basename "$mig") pode já ter sido aplicada"
    done
    ok "migrations aplicadas"
  else
    info "pasta migrations sem arquivos .sql"
  fi
else
  info "pasta migrations não encontrada"
fi

# ══════════════════════════════════════════════════════════════════════════════
# 4. ELAI-SERVER
# ══════════════════════════════════════════════════════════════════════════════
if [[ "${SKIP_SERVER:-}" != "1" ]]; then
  info "iniciando elai-server em :${SERVER_PORT}..."

  # mata processo anterior na porta (macOS-safe)
  if command -v lsof > /dev/null 2>&1; then
    lsof -ti ":${SERVER_PORT}" 2>/dev/null | xargs kill -9 2>/dev/null || true
  elif command -v fuser > /dev/null 2>&1; then
    fuser -k "${SERVER_PORT}/tcp" 2>/dev/null || true
  fi

  # compila se necessário
  SERVER_BINARY="${RUST_DIR}target/debug/elai-server"
  mkdir -p "${SERVER_DIR}/tmp"
  if [[ ! -f "$SERVER_BINARY" ]]; then
    warn "binário não encontrado — compilando (pode demorar na primeira vez)..."
    cd "$RUST_DIR"
    cargo build --package server 2>&1 | tag_stream "$C_SERVER" "[SERVER build]"
  fi

  # sobe em background com saída colorida ao vivo
  (
    set -a
    [[ -f "$ENV_FILE" ]] && source "$ENV_FILE" 2>/dev/null || true
    set +a
    PORT="${SERVER_PORT}" RUST_BACKTRACE=1 "$SERVER_BINARY" 2>&1
  ) | tag_stream "$C_SERVER" "[SERVER :${SERVER_PORT}]" &
  SERVER_PID=$!
  echo $SERVER_PID > "${SERVER_DIR}/tmp/server.pid"
  ALL_PIDS+=("$SERVER_PID")

  info "server PID=${SERVER_PID} — aguardando health check..."
  sleep 4

  for i in $(seq 1 15); do
    if curl -sf "http://127.0.0.1:${SERVER_PORT}/v1/health" > /dev/null 2>&1; then
      ok "elai-server responding em http://127.0.0.1:${SERVER_PORT}"
      break
    fi
    sleep 1
  done

  if ! curl -sf "http://127.0.0.1:${SERVER_PORT}/v1/health" > /dev/null 2>&1; then
    warn "server não respondeu health check — verifique os logs acima"
  fi
else
  info "SKIP_SERVER=1 — server não foi iniciado"
fi

# ══════════════════════════════════════════════════════════════════════════════
# 5. FRONTEND (vite / next / react)
# ══════════════════════════════════════════════════════════════════════════════
if [[ -z "${FRONT_DIR}" ]]; then
  info "frontend não configurado — pulando"
else
  info "iniciando frontend em $FRONT_DIR ..."

  mkdir -p "${SERVER_DIR}/tmp"

  # detecta qual bundler
  if [[ -f "${FRONT_DIR}/package.json" ]]; then
    if grep -q '"next"' "${FRONT_DIR}/package.json"; then
      FRONT_NAME="Next.js"
    elif grep -q '"vite"' "${FRONT_DIR}/package.json" || \
         grep -q '"react"' "${FRONT_DIR}/package.json"; then
      FRONT_NAME="Vite/React"
    else
      FRONT_NAME="npm dev"
    fi
  else
    warn "package.json não encontrado em $FRONT_DIR — pulando frontend"
    FRONT_DIR=""
  fi

  if [[ -n "${FRONT_DIR}" ]]; then
    (
      cd "$FRONT_DIR"
      npm run dev 2>&1
    ) | tag_stream "$C_FRONT" "[FRONT  :${FRONT_PORT}]" &
    FRONT_PID=$!
    echo $FRONT_PID > "${SERVER_DIR}/tmp/front.pid"
    ALL_PIDS+=("$FRONT_PID")

    info "frontend PID=${FRONT_PID} — aguardando subir..."
    for i in $(seq 1 20); do
      if nc -z localhost "$FRONT_PORT" 2>/dev/null; then
        ok "frontend responding em http://127.0.0.1:${FRONT_PORT}"
        break
      fi
      sleep 1
    done

    if ! nc -z localhost "$FRONT_PORT" 2>/dev/null; then
      warn "frontend pode não ter subido — verifique os logs acima"
    fi
  fi
fi

# ══════════════════════════════════════════════════════════════════════════════
# 6. DOCS SERVER + MULTIPLEXED FOREGROUND WAIT
# ══════════════════════════════════════════════════════════════════════════════
DOCS_DIR="${SERVER_DIR}/docs/site"

echo ""
echo -e "${BLD}─────────────────────────────────────────────────────────${RES}"
echo -e "  ${GRN}✓ tudo no ar!${RES}"
echo ""
echo -e "  📦 infraestrutura:"
echo -e "     PostgreSQL  ${GRN}localhost:${PG_PORT}${RES}  (${PG_DB})"
echo -e "     Qdrant      ${GRN}localhost:${QDRANT_PORT}${RES}  (HTTP + ${QDRANT_GRPC_PORT} gRPC)"
echo -e "     Redis       ${GRN}localhost:${REDIS_PORT}${RES}"
echo ""
echo -e "  ${C_SERVER}●${RES} elai-server  ${CYA}http://127.0.0.1:${SERVER_PORT}${RES}"
if [[ -n "${FRONT_DIR:-}" ]]; then
  echo -e "  ${C_FRONT}●${RES} frontend     ${MAG}http://127.0.0.1:${FRONT_PORT}${RES}"
fi
if [[ "${SKIP_DOCS:-}" != "1" ]] && [[ -d "$DOCS_DIR" ]]; then
  echo -e "  ${C_DOCS}●${RES} docs         ${BLU}http://127.0.0.1:${DOCS_PORT}${RES}"
fi
echo ""
echo -e "  🔗 DATABASE_URL=${DATABASE_URL}"
echo -e "─────────────────────────────────────────────────────────${RES}"
echo ""

# ── cleanup ──────────────────────────────────────────────────────────────────
cleanup() {
  echo ""
  info "encerrando processos..."
  for pid_file in "${SERVER_DIR}/tmp/server.pid" "${SERVER_DIR}/tmp/front.pid"; do
    if [[ -f "$pid_file" ]]; then
      local pid
      pid=$(cat "$pid_file" 2>/dev/null)
      [[ -n "$pid" ]] && { kill -- "-${pid}" 2>/dev/null || kill "$pid" 2>/dev/null || true; }
      rm -f "$pid_file"
    fi
  done
  [[ -n "${DOCS_PID:-}" ]] && { kill -- "-${DOCS_PID}" 2>/dev/null || kill "$DOCS_PID" 2>/dev/null || true; }
  ok "feito"
}
trap cleanup EXIT INT TERM

# docs server em background com saída colorida (exceto se SKIP_DOCS ou pasta ausente)
DOCS_PID=""
if [[ "${SKIP_DOCS:-}" != "1" ]]; then
  if [[ -d "$DOCS_DIR" ]]; then
    # libera a porta antes de tentar subir
    lsof -ti ":${DOCS_PORT}" 2>/dev/null | xargs kill -9 2>/dev/null || true
    if command -v python3 > /dev/null 2>&1; then
      python3 -m http.server "$DOCS_PORT" --bind 127.0.0.1 2>&1 | \
        tag_stream "$C_DOCS" "[DOCS   :${DOCS_PORT}]" &
      DOCS_PID=$!
      ALL_PIDS+=("$DOCS_PID")
    elif command -v npx > /dev/null 2>&1; then
      npx --yes http-server -p "$DOCS_PORT" -a 127.0.0.1 -c-1 "$DOCS_DIR" 2>&1 | \
        tag_stream "$C_DOCS" "[DOCS   :${DOCS_PORT}]" &
      DOCS_PID=$!
      ALL_PIDS+=("$DOCS_PID")
    else
      warn "sem python3 nem npx — docs não serão servidas"
    fi
  else
    warn "docs/site não encontrada — pulando servidor de docs"
  fi
fi

if [[ ${#ALL_PIDS[@]} -eq 0 ]]; then
  warn "nenhum processo em foreground — saindo"
  exit 0
fi

info "logs ao vivo (Ctrl+C para encerrar tudo):"
echo -e "  ${C_SERVER}[SERVER :${SERVER_PORT}]${RES}  ${C_FRONT}[FRONT  :${FRONT_PORT}]${RES}  ${C_DOCS}[DOCS   :${DOCS_PORT}]${RES}"
echo ""

wait "${ALL_PIDS[@]}"