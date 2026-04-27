#!/usr/bin/env bash
# scripts/release.sh <versão>
# Prepara e publica uma release localmente:
#   1. Bumpa a versão no Cargo.toml
#   2. Atualiza Cargo.lock
#   3. Gera changelog desde a última tag
#   4. Atualiza a seção "What's New" do README.md
#   5. Commita tudo
#   6. Cria a tag anotada
#   7. Faz push do commit + tag
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ── Argumento ─────────────────────────────────────────────────────────────────
NEW_VERSION="${1:-}"
if [ -z "$NEW_VERSION" ]; then
    echo "Uso: $0 <versão>"
    echo "Exemplo: $0 0.4.7"
    exit 1
fi
NEW_VERSION="${NEW_VERSION#v}"
TAG="v${NEW_VERSION}"

# ── Pré-verificações ──────────────────────────────────────────────────────────
BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$BRANCH" != "main" ]; then
    echo "❌ Você precisa estar na branch main (atual: $BRANCH)"
    exit 1
fi

if ! git diff --quiet || ! git diff --staged --quiet; then
    echo "❌ Há alterações não commitadas. Faça commit antes de publicar."
    exit 1
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "❌ Tag $TAG já existe."
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "❌ python3 não encontrado."
    exit 1
fi

echo "🚀 Preparando release $TAG..."

# ── 1. Bump Cargo.toml ────────────────────────────────────────────────────────
CARGO_TOML="rust/Cargo.toml"
CURRENT_VERSION=$(grep '^version = ' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/')
echo "  versão: $CURRENT_VERSION → $NEW_VERSION"

python3 - <<PYEOF
with open('$CARGO_TOML') as f:
    content = f.read()
updated = content.replace('version = "$CURRENT_VERSION"', 'version = "$NEW_VERSION"', 1)
with open('$CARGO_TOML', 'w') as f:
    f.write(updated)
PYEOF

# ── 2. Atualiza Cargo.lock ────────────────────────────────────────────────────
echo "  atualizando Cargo.lock..."
(cd rust && cargo check --quiet > /dev/null 2>&1)

# ── 3. Gera changelog ─────────────────────────────────────────────────────────
echo "  gerando changelog..."
PREV=$(git tag --sort=-version:refname | head -1 2>/dev/null || true)

if [ -n "$PREV" ]; then
    RAW=$(git log "${PREV}..HEAD" --pretty=format:"%s" --no-merges 2>/dev/null || true)
else
    RAW=$(git log HEAD --pretty=format:"%s" --no-merges --max-count=20 2>/dev/null || true)
fi

FILTERED=$(printf '%s\n' "$RAW" \
    | grep -vE "^(chore: bump version|Merge |docs: atualiza README)" \
    | grep -v "^$" \
    || true)

CHANGELOG=$(printf '%s\n' "$FILTERED" | sed 's/^/- /' || true)
[ -z "$CHANGELOG" ] && CHANGELOG="- Maintenance release"

echo "  changelog:"
printf '%s\n' "$CHANGELOG" | sed 's/^/    /'

# ── 4. Atualiza README ────────────────────────────────────────────────────────
echo "  atualizando README.md..."
CHANGELOG_TMP=$(mktemp)
printf '%s\n' "$CHANGELOG" > "$CHANGELOG_TMP"

python3 - "$TAG" "$CHANGELOG_TMP" <<'PYEOF'
import re, sys

tag = sys.argv[1]
changelog_file = sys.argv[2]

with open(changelog_file) as f:
    changelog = f.read().strip()

with open('README.md') as f:
    content = f.read()

new_section = (
    f"## What's New — {tag}\n\n"
    f"{changelog}\n\n"
    f"---"
)

updated = re.sub(
    r"## What's New — v[\d.]+.*?\n---",
    new_section,
    content,
    count=1,
    flags=re.DOTALL,
)

if updated == content:
    print("  ⚠  Seção 'What's New' não encontrada no README — nada alterado.")
else:
    with open('README.md', 'w') as f:
        f.write(updated)
    print("  README.md atualizado.")
PYEOF

rm -f "$CHANGELOG_TMP"

# ── 5. Commit ─────────────────────────────────────────────────────────────────
echo "  commitando..."
git add rust/Cargo.toml rust/Cargo.lock README.md
git commit -m "chore: bump version to ${TAG}"

# ── 6. Tag anotada ────────────────────────────────────────────────────────────
git tag -a "$TAG" -m "Release ${TAG}"
echo "  tag $TAG criada."

# ── 7. Push ───────────────────────────────────────────────────────────────────
echo "  publicando..."
git push origin main --follow-tags

echo ""
echo "✅ Release $TAG publicado!"
echo "   CI: https://github.com/nextlw/elai-code/actions"
