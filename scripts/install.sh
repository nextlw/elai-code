#!/usr/bin/env sh
set -eu

REPO="nextlw/elai-code"
BIN_NAME="elai"
INSTALL_DIR="${ELAI_INSTALL_DIR:-/usr/local/bin}"
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

# ── Step 1: Provider selection ────────────────────────────────────────────────
printf "  ${BOLD}Step 1 — Choose your AI provider${RESET}\n\n"
printf "    [1] Anthropic  (Claude opus / sonnet / haiku)\n"
printf "    [2] OpenAI     (gpt-4o, gpt-4o-mini, o3…)\n"
printf "    [3] Both\n\n"
printf "  Choice [1]: "
read -r PROVIDER_CHOICE
PROVIDER_CHOICE="${PROVIDER_CHOICE:-1}"

ANTHROPIC_KEY=""
OPENAI_KEY=""

read_secret() {
  prompt="$1"
  if [ -t 0 ]; then
    printf "  %s" "$prompt"
    stty -echo 2>/dev/null || true
    read -r SECRET
    stty echo 2>/dev/null || true
    printf "\n"
  else
    printf "  %s" "$prompt"
    read -r SECRET
  fi
  printf '%s' "$SECRET"
}

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
    error "Invalid choice: $PROVIDER_CHOICE" ;;
esac

# ── Step 2: Download binary ───────────────────────────────────────────────────
printf "\n  ${BOLD}Step 2 — Installing elai binary${RESET}\n\n"
say "Downloading ${TARGET}..."

URL="https://github.com/${REPO}/releases/latest/download/${TARGET}"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$URL" -o "$TMP"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$TMP" "$URL"
else
  error "curl or wget is required."
fi

chmod +x "$TMP"

if [ -w "$INSTALL_DIR" ]; then
  mv "$TMP" "${INSTALL_DIR}/${BIN_NAME}"
else
  say "Installing to ${INSTALL_DIR} (sudo required)..."
  sudo mv "$TMP" "${INSTALL_DIR}/${BIN_NAME}"
fi

ok "Binary installed → ${INSTALL_DIR}/${BIN_NAME}"

# ── Step 3: Save API keys ─────────────────────────────────────────────────────
printf "\n  ${BOLD}Step 3 — Saving API keys${RESET}\n\n"

mkdir -p "$ELAI_DIR"

# Write ~/.elai/.env (read by elai on every run)
{
  printf "# Elai Code — API keys\n"
  [ -n "$ANTHROPIC_KEY" ] && printf "ANTHROPIC_API_KEY=%s\n" "$ANTHROPIC_KEY"
  [ -n "$OPENAI_KEY" ]    && printf "OPENAI_API_KEY=%s\n"    "$OPENAI_KEY"
} > "$ENV_FILE"
chmod 600 "$ENV_FILE"
ok "Keys saved to ${ENV_FILE}"

# Also export in the user's shell RC
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
  warn "Shell config already contains elai keys — update ${SHELL_RC} manually if needed."
else
  {
    printf "\n%s\n" "$MARKER"
    [ -n "$ANTHROPIC_KEY" ] && printf 'export ANTHROPIC_API_KEY="%s"\n' "$ANTHROPIC_KEY"
    [ -n "$OPENAI_KEY" ]    && printf 'export OPENAI_API_KEY="%s"\n'    "$OPENAI_KEY"
  } >> "$SHELL_RC"
  ok "Keys exported in ${SHELL_RC}"
fi

# ── Done ──────────────────────────────────────────────────────────────────────
printf "\n  ${GREEN}${BOLD}Installation complete!${RESET}\n\n"
printf "  Reload your shell or run:\n\n"
printf "    ${BOLD}source %s${RESET}\n\n" "$SHELL_RC"
printf "  Then start Elai with:\n\n"
printf "    ${BOLD}elai${RESET}\n\n"
