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
EXISTING_KEYS=false

if command -v elai >/dev/null 2>&1; then
  CURRENT_VERSION="$(elai --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
fi

if [ -f "$ENV_FILE" ] && grep -qE "^(ANTHROPIC|OPENAI)_API_KEY=.+" "$ENV_FILE" 2>/dev/null; then
  EXISTING_KEYS=true
fi

IS_UPDATE=false
if [ -n "$CURRENT_VERSION" ]; then
  IS_UPDATE=true
  printf "  ${BOLD}Instalação existente detectada: v${CURRENT_VERSION}${RESET}\n\n"
fi

# ── Step 1: Provider / API keys ───────────────────────────────────────────────
printf "  ${BOLD}Step 1 — API keys${RESET}\n\n"

ANTHROPIC_KEY=""
OPENAI_KEY=""
SKIP_KEYS=false

read_secret() {
  prompt="$1"
  # Write prompt and newline to /dev/tty so they are NOT captured when the
  # caller uses $(...) command substitution — only the secret goes to stdout.
  printf "  %s" "$prompt" >/dev/tty
  stty -echo </dev/tty 2>/dev/null || true
  read -r SECRET </dev/tty
  stty echo </dev/tty 2>/dev/null || true
  printf "\n" >/dev/tty
  printf '%s' "$SECRET"
}

if "$EXISTING_KEYS"; then
  printf "  Chaves já configuradas em %s\n" "$ENV_FILE"
  printf "  Atualizar chaves? [y/N]: "
  read -r UPDATE_KEYS </dev/tty
  UPDATE_KEYS="${UPDATE_KEYS:-n}"
  case "$UPDATE_KEYS" in
    [Yy]*) SKIP_KEYS=false ;;
    *)     SKIP_KEYS=true; ok "Mantendo chaves existentes." ;;
  esac
fi

if ! "$SKIP_KEYS"; then
  printf "\n    [1] Anthropic  (Claude opus / sonnet / haiku)\n"
  printf "    [2] OpenAI     (gpt-4o, gpt-4o-mini, o3…)\n"
  printf "    [3] Ambos\n\n"
  printf "  Escolha [1]: "
  read -r PROVIDER_CHOICE </dev/tty
  PROVIDER_CHOICE="${PROVIDER_CHOICE:-1}"

  case "$PROVIDER_CHOICE" in
    1)
      ANTHROPIC_KEY="$(read_secret 'Anthropic API key: ')"
      [ -z "$ANTHROPIC_KEY" ] && error "API key cannot be empty." ;;
    2)
      OPENAI_KEY="$(read_secret 'OpenAI API key: ')"
      [ -z "$OPENAI_KEY" ] && error "API key cannot be empty." ;;
    3)
      ANTHROPIC_KEY="$(read_secret 'Anthropic API key: ')"
      [ -z "$ANTHROPIC_KEY" ] && error "Anthropic API key cannot be empty."
      OPENAI_KEY="$(read_secret 'OpenAI API key: ')"
      [ -z "$OPENAI_KEY" ] && error "OpenAI API key cannot be empty." ;;
    *)
      error "Escolha inválida: $PROVIDER_CHOICE" ;;
  esac
fi

# ── Step 2: Download binary ───────────────────────────────────────────────────
printf "\n  ${BOLD}Step 2 — Instalando binário${RESET}\n\n"

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

# ── Step 3: Save API keys ─────────────────────────────────────────────────────
if ! "$SKIP_KEYS"; then
  printf "\n  ${BOLD}Step 3 — Salvando API keys${RESET}\n\n"

  mkdir -p "$ELAI_DIR"

  {
    printf "# Elai Code — API keys\n"
    [ -n "$ANTHROPIC_KEY" ] && printf "ANTHROPIC_API_KEY=%s\n" "$ANTHROPIC_KEY"
    [ -n "$OPENAI_KEY" ]    && printf "OPENAI_API_KEY=%s\n"    "$OPENAI_KEY"
  } > "$ENV_FILE"
  chmod 600 "$ENV_FILE"
  ok "Chaves salvas em ${ENV_FILE}"

  # Detect shell RC file.
  SHELL_RC=""
  case "${SHELL:-}" in
    */zsh)  SHELL_RC="${HOME}/.zshrc" ;;
    */bash) SHELL_RC="${HOME}/.bashrc" ;;
    */fish) SHELL_RC="${HOME}/.config/fish/config.fish" ;;
    *)
      [ -f "${HOME}/.zshrc" ]  && SHELL_RC="${HOME}/.zshrc"  || \
      [ -f "${HOME}/.bashrc" ] && SHELL_RC="${HOME}/.bashrc" || \
      SHELL_RC="${HOME}/.profile" ;;
  esac

  MARKER="# elai-code api keys"
  if [ -f "$SHELL_RC" ] && grep -q "$MARKER" "$SHELL_RC" 2>/dev/null; then
    # Block already present — update the values in-place using a temp file.
    TMP_RC="$(mktemp)"
    awk -v anth="$ANTHROPIC_KEY" -v oai="$OPENAI_KEY" -v marker="$MARKER" '
      $0 == marker { in_block=1; print; next }
      in_block && /^export ANTHROPIC_API_KEY=/ {
        if (anth != "") print "export ANTHROPIC_API_KEY=\"" anth "\""
        next
      }
      in_block && /^export OPENAI_API_KEY=/ {
        if (oai != "") print "export OPENAI_API_KEY=\"" oai "\""
        next
      }
      in_block && /^$/ { in_block=0 }
      { print }
    ' "$SHELL_RC" > "$TMP_RC" && mv "$TMP_RC" "$SHELL_RC"
    ok "Chaves atualizadas em ${SHELL_RC}"
  else
    {
      printf "\n%s\n" "$MARKER"
      [ -n "$ANTHROPIC_KEY" ] && printf 'export ANTHROPIC_API_KEY="%s"\n' "$ANTHROPIC_KEY"
      [ -n "$OPENAI_KEY" ]    && printf 'export OPENAI_API_KEY="%s"\n'    "$OPENAI_KEY"
    } >> "$SHELL_RC"
    ok "Chaves exportadas em ${SHELL_RC}"
  fi
fi

# ── Done ──────────────────────────────────────────────────────────────────────
printf "\n  ${GREEN}${BOLD}"
if "$IS_UPDATE"; then
  printf "Atualização concluída!"
else
  printf "Instalação concluída!"
fi
printf "${RESET}\n\n"

if ! "$SKIP_KEYS"; then
  SHELL_RC="${SHELL_RC:-${HOME}/.zshrc}"
  printf "  Recarregue o shell ou execute:\n\n"
  printf "    ${BOLD}source %s${RESET}\n\n" "$SHELL_RC"
fi

printf "  Inicie o Elai com:\n\n"
printf "    ${BOLD}elai${RESET}\n\n"
