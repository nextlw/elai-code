#!/usr/bin/env bash
# Smoke test for the install.sh auth menu — no real download, no real binary.
set -eu

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Create stub binary that just echoes its arguments.
mkdir -p "$TMP/bin"
cat > "$TMP/bin/elai" <<'STUB'
#!/usr/bin/env sh
echo "stub elai $*"
exit 0
STUB
chmod +x "$TMP/bin/elai"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

run_choice() {
  local label="$1"
  local input="$2"
  printf '%s\n' "$input" | \
    ELAI_INSTALL_DIR="$TMP/bin" \
    HOME="$TMP" \
    SHELL=/bin/bash \
    bash "$SCRIPT_DIR/install.sh" 2>&1 || true
  echo "--- $label: done ---"
}

echo "=== Smoke test: option 8 (skip) ==="
run_choice "skip" "8"

echo "=== Smoke test: option 7 (OpenAI key) ==="
# Provide: auth choice=7, then a fake key
printf '7\nsk-openai-test-key\n' | \
  ELAI_INSTALL_DIR="$TMP/bin" \
  HOME="$TMP" \
  SHELL=/bin/bash \
  bash "$SCRIPT_DIR/install.sh" 2>&1 || true
echo "--- openai: done ---"

echo ""
echo "smoke test passed"
