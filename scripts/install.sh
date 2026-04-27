#!/usr/bin/env sh
set -eu

REPO="nextlw/elai-code"
BIN_NAME="elai"

# Prefer the Homebrew bin prefix on macOS arm64 so the binary lands ahead of
# any stale files in /usr/local/bin when Homebrew is on the PATH.
_default_install_dir() {
  case "$(uname -s):$(uname -m)" in
    Darwin:arm64)
      if [ -d "/opt/homebrew/bin" ]; then printf '/opt/homebrew/bin'; return; fi ;;
    Darwin:*)
      if command -v brew >/dev/null 2>&1; then printf '%s/bin' "$(brew --prefix)"; return; fi ;;
  esac
  printf '/usr/local/bin'
}

INSTALL_DIR="${ELAI_INSTALL_DIR:-$(_default_install_dir)}"
ELAI_DIR="${HOME}/.elai"
ENV_FILE="${ELAI_DIR}/.env"

# ── Colors ────────────────────────────────────────────────────────────────────
if [ -t 1 ]; then
  BOLD='\033[1m'; CYAN='\033[0;36m'; GREEN='\033[0;32m'
  YELLOW='\033[0;33m'; RED='\033[0;31m'; RESET='\033[0m'
else
  BOLD=''; CYAN=''; GREEN=''; YELLOW=''; RED=''; RESET=''
fi

say()   { printf "  ${CYAN}▶${RESET} %s\n" "$1"; }
ok()    { printf "  ${GREEN}✓${RESET} %s\n" "$1"; }
warn()  { printf "  ${YELLOW}!${RESET} %s\n" "$1"; }
error() { printf "  ${RED}✗${RESET} %s\n" "$1" >&2; exit 1; }

# ── Banner ────────────────────────────────────────────────────────────────────
printf "\n${BOLD}"
printf "  ██████████████████   ███████╗██╗      █████╗ ██╗\n"
printf "  ████████  ▄▄  ▄▄     ██╔════╝██║     ██╔══██╗██║\n"
printf "  ████████  ██  ██     █████╗  ██║     ███████║██║\n"
printf "  ████████  ▀▀  ▀▀     ██╔══╝  ██║     ██╔══██║██║\n"
printf "  ██████████████████   ███████╗███████╗██║  ██║██║\n"
printf "        ████  ████     ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝\n"
printf "${RESET}\n"
printf "  Elai Code Installer\n\n"

# ── Detect OS / arch ──────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64)  TARGET="elai-macos-arm64" ;;
      x86_64) TARGET="elai-macos-x86_64" ;;
      *) error "Unsupported architecture: $ARCH" ;;
    esac ;;
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="elai-linux-x86_64" ;;
      aarch64) TARGET="elai-linux-arm64" ;;
      *) error "Unsupported architecture: $ARCH" ;;
    esac ;;
  *)
    error "Unsupported OS: $OS. For Windows run: irm https://raw.githubusercontent.com/${REPO}/main/scripts/install.ps1 | iex" ;;
esac

# ── Detect existing installation ──────────────────────────────────────────────
CURRENT_VERSION=""

if command -v elai >/dev/null 2>&1; then
  CURRENT_VERSION="$(elai --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
fi

IS_UPDATE=false
if [ -n "$CURRENT_VERSION" ]; then
  IS_UPDATE=true
  printf "  ${BOLD}Instalação existente detectada: v${CURRENT_VERSION}${RESET}\n\n"
fi

# ── Helper: read a secret without echo ────────────────────────────────────────
read_secret() {
  prompt="$1"
  # Write prompt to /dev/tty so it is NOT captured by $(...) substitution.
  printf "  %s" "$prompt" >/dev/tty
  stty -echo </dev/tty 2>/dev/null || true
  read -r SECRET </dev/tty
  stty echo </dev/tty 2>/dev/null || true
  printf "\n" >/dev/tty
  printf '%s' "$SECRET"
}

# ── Detect shell RC file ───────────────────────────────────────────────────────
detect_shell_rc() {
  case "${SHELL:-}" in
    */zsh)  printf '%s/.zshrc' "$HOME" ;;
    */bash) printf '%s/.bashrc' "$HOME" ;;
    */fish) printf '%s/.config/fish/config.fish' "$HOME" ;;
    *)
      if [ -f "${HOME}/.zshrc" ]; then
        printf '%s/.zshrc' "$HOME"
      elif [ -f "${HOME}/.bashrc" ]; then
        printf '%s/.bashrc' "$HOME"
      else
        printf '%s/.profile' "$HOME"
      fi ;;
  esac
}

SHELL_RC="$(detect_shell_rc)"

# ── Step 1: Download binary ───────────────────────────────────────────────────
printf "  ${BOLD}Step 1 — Instalando binário${RESET}\n\n"

# Fetch latest version tag from GitHub API to skip download if already current.
LATEST_VERSION=""
if command -v curl >/dev/null 2>&1; then
  LATEST_VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
    | grep '"tag_name"' | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
fi

if [ -n "$CURRENT_VERSION" ] && [ -n "$LATEST_VERSION" ] && [ "$CURRENT_VERSION" = "$LATEST_VERSION" ]; then
  ok "Binário já está na versão mais recente (v${CURRENT_VERSION}). Nada a fazer."
else
  if [ -n "$LATEST_VERSION" ]; then
    say "Baixando elai v${LATEST_VERSION} (${TARGET})..."
  else
    say "Baixando ${TARGET}..."
  fi

  URL="https://github.com/${REPO}/releases/latest/download/${TARGET}"
  TMP="$(mktemp)"
  trap 'rm -f "$TMP"' EXIT

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$URL" -o "$TMP"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$TMP" "$URL"
  else
    error "curl ou wget é necessário."
  fi

  chmod +x "$TMP"

  if [ -w "$INSTALL_DIR" ]; then
    mv "$TMP" "${INSTALL_DIR}/${BIN_NAME}"
  else
    say "Instalando em ${INSTALL_DIR} (sudo necessário)..."
    sudo mv "$TMP" "${INSTALL_DIR}/${BIN_NAME}"
  fi

  ok "Binário instalado → ${INSTALL_DIR}/${BIN_NAME}"
fi

ELAI_BIN="${INSTALL_DIR}/${BIN_NAME}"

# ── Step 2: Authentication ────────────────────────────────────────────────────
printf "\n  ${BOLD}Step 2 — Authentication${RESET}\n\n"

# If this is an update, ask whether to reconfigure auth.
if "$IS_UPDATE"; then
  printf "  Atualizar autenticação? [y/N]: "
  read -r UPDATE_AUTH </dev/tty
  UPDATE_AUTH="${UPDATE_AUTH:-n}"
  case "$UPDATE_AUTH" in
    [Yy]*) : ;;  # fall through to menu
    *)
      ok "Mantendo autenticação existente."
      printf "\n  ${GREEN}${BOLD}Atualização concluída!${RESET}\n\n"
      printf "  Inicie o Elai com:\n\n"
      printf "    ${BOLD}elai${RESET}\n\n"
      printf "  Para trocar o método de auth depois:\n\n"
      printf "    ${BOLD}elai login --claudeai|--console|--sso|--api-key|--token|--use-bedrock|...${RESET}\n"
      printf "    ${BOLD}elai auth status${RESET}     # ver método ativo\n"
      printf "    ${BOLD}elai auth list${RESET}       # ver todos os métodos\n\n"
      exit 0
      ;;
  esac
fi

# Detect existing Claude Code credentials and show hint.
_has_claude_creds=false
if [ -f "${HOME}/.claude/.credentials.json" ]; then
  _has_claude_creds=true
elif [ "$OS" = "Darwin" ]; then
  if security find-generic-password -s "Claude Code-credentials" -w >/dev/null 2>&1; then
    _has_claude_creds=true
  fi
fi

if "$_has_claude_creds"; then
  printf "  ${YELLOW}!${RESET} Credenciais Claude Code detectadas. Para importá-las, após a instalação:\n"
  printf "       ${BOLD}elai login --import-claude-code${RESET}   (em breve)\n"
  printf "     Ou escolha [1] para fazer um novo login.\n\n"
fi

# Display auth menu.
printf "  How would you like to authenticate?\n\n"
printf "    [1] Claude Pro/Max — log in to claude.ai (recommended)\n"
printf "    [2] Anthropic Console — generate an API key via OAuth\n"
printf "    [3] SSO (asks for e-mail)\n"
printf "    [4] Paste an Anthropic API key (sk-ant-...)\n"
printf "    [5] Paste an ANTHROPIC_AUTH_TOKEN\n"
printf "    [6] AWS Bedrock / Google Vertex / Azure Foundry\n"
printf "    [7] OpenAI only (no Anthropic) — keys go to ~/.elai/.env\n"
printf "    [8] Skip — configure later with \`elai login\`\n\n"
printf "  Choose [1]: "
read -r AUTH_CHOICE </dev/tty
AUTH_CHOICE="${AUTH_CHOICE:-1}"

case "$AUTH_CHOICE" in
  1)
    # Claude Pro/Max — OAuth claude.ai
    say "Opening claude.ai login..."
    "$ELAI_BIN" login --claudeai
    ok "Authentication via claude.ai complete."
    ;;

  2)
    # Anthropic Console — OAuth
    say "Opening Anthropic Console login..."
    "$ELAI_BIN" login --console
    ok "Authentication via Anthropic Console complete."
    ;;

  3)
    # SSO
    printf "  E-mail SSO: " >/dev/tty
    read -r SSO_EMAIL </dev/tty
    [ -z "$SSO_EMAIL" ] && error "E-mail cannot be empty."
    say "Starting SSO login for ${SSO_EMAIL}..."
    "$ELAI_BIN" login --sso --email "$SSO_EMAIL"
    ok "SSO authentication complete."
    ;;

  4)
    # Paste Anthropic API key
    ANTHROPIC_KEY="$(read_secret 'Anthropic API key (sk-ant-...): ')"
    [ -z "$ANTHROPIC_KEY" ] && error "API key cannot be empty."
    printf '%s\n' "$ANTHROPIC_KEY" | "$ELAI_BIN" login --api-key --stdin
    ok "API key saved."
    ;;

  5)
    # Paste ANTHROPIC_AUTH_TOKEN
    AUTH_TOKEN="$(read_secret 'ANTHROPIC_AUTH_TOKEN: ')"
    [ -z "$AUTH_TOKEN" ] && error "Auth token cannot be empty."
    printf '%s\n' "$AUTH_TOKEN" | "$ELAI_BIN" login --token --stdin
    ok "Auth token saved."
    ;;

  6)
    # Third-party: Bedrock / Vertex / Foundry
    printf "\n    [a] AWS Bedrock\n"
    printf "    [b] Google Vertex\n"
    printf "    [c] Azure Foundry\n\n"
    printf "  Choose [a]: "
    read -r THREE_P_CHOICE </dev/tty
    THREE_P_CHOICE="${THREE_P_CHOICE:-a}"

    case "$THREE_P_CHOICE" in
      a|A)
        THREE_P_FLAG="--use-bedrock"
        THREE_P_VAR="CLAUDE_CODE_USE_BEDROCK"
        ;;
      b|B)
        THREE_P_FLAG="--use-vertex"
        THREE_P_VAR="CLAUDE_CODE_USE_VERTEX"
        ;;
      c|C)
        THREE_P_FLAG="--use-foundry"
        THREE_P_VAR="CLAUDE_CODE_USE_FOUNDRY"
        ;;
      *)
        error "Escolha inválida: $THREE_P_CHOICE"
        ;;
    esac

    "$ELAI_BIN" login "$THREE_P_FLAG"

    printf "  Adicionar 'export %s=1' em %s? [y/N]: " "$THREE_P_VAR" "$SHELL_RC" >/dev/tty
    read -r ADD_ENV </dev/tty
    ADD_ENV="${ADD_ENV:-n}"
    case "$ADD_ENV" in
      [Yy]*)
        printf '\nexport %s=1\n' "$THREE_P_VAR" >> "$SHELL_RC"
        ok "Adicionado 'export ${THREE_P_VAR}=1' em ${SHELL_RC}"
        ;;
      *)
        warn "Variável não adicionada ao shell RC. Adicione manualmente se necessário."
        ;;
    esac
    ;;

  7)
    # OpenAI only
    OPENAI_KEY="$(read_secret 'OpenAI API key: ')"
    [ -z "$OPENAI_KEY" ] && error "API key cannot be empty."

    mkdir -p "$ELAI_DIR"
    {
      printf "# Elai Code — API keys\n"
      printf "OPENAI_API_KEY=%s\n" "$OPENAI_KEY"
    } > "$ENV_FILE"
    chmod 600 "$ENV_FILE"
    ok "OpenAI API key salva em ${ENV_FILE}"

    MARKER="# elai-code api keys"
    if [ -f "$SHELL_RC" ] && grep -q "$MARKER" "$SHELL_RC" 2>/dev/null; then
      TMP_RC="$(mktemp)"
      awk -v oai="$OPENAI_KEY" -v marker="$MARKER" '
        $0 == marker { in_block=1; print; next }
        in_block && /^export OPENAI_API_KEY=/ {
          print "export OPENAI_API_KEY=\"" oai "\""
          next
        }
        in_block && /^$/ { in_block=0 }
        { print }
      ' "$SHELL_RC" > "$TMP_RC" && mv "$TMP_RC" "$SHELL_RC"
      ok "Chave atualizada em ${SHELL_RC}"
    else
      {
        printf "\n%s\n" "$MARKER"
        printf 'export OPENAI_API_KEY="%s"\n' "$OPENAI_KEY"
      } >> "$SHELL_RC"
      ok "Chave exportada em ${SHELL_RC}"
    fi
    ;;

  8)
    warn "Pulando autenticação. Use 'elai login' para configurar depois."
    ;;

  *)
    error "Escolha inválida: $AUTH_CHOICE"
    ;;
esac

# ── Done ──────────────────────────────────────────────────────────────────────
printf "\n  ${GREEN}${BOLD}"
if "$IS_UPDATE"; then
  printf "Atualização concluída!"
else
  printf "Instalação concluída!"
fi
printf "${RESET}\n\n"

printf "  Inicie o Elai com:\n\n"
printf "    ${BOLD}elai${RESET}\n\n"
printf "  Para trocar o método de auth depois:\n\n"
printf "    ${BOLD}elai login --claudeai|--console|--sso|--api-key|--token|--use-bedrock|...${RESET}\n"
printf "    ${BOLD}elai auth status${RESET}     # ver método ativo\n"
printf "    ${BOLD}elai auth list${RESET}       # ver todos os métodos\n\n"

if [ "$AUTH_CHOICE" = "7" ]; then
  printf "  Recarregue o shell ou execute:\n\n"
  printf "    ${BOLD}source %s${RESET}\n\n" "$SHELL_RC"
fi
