#!/usr/bin/env bash
# scripts/bump.sh <major|minor|patch>
# Incrementa a versão no Cargo.toml seguindo semver:
#   bump.sh patch  → 1.1.3 → 1.1.4
#   bump.sh minor  → 1.1.3 → 1.2.0
#   bump.sh major  → 1.1.3 → 2.0.0
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ── Argumento ─────────────────────────────────────────────────────────────────
LEVEL="${1:-}"
if [[ ! "$LEVEL" =~ ^(major|minor|patch)$ ]]; then
    echo "Uso: $0 <major|minor|patch>"
    echo ""
    echo "Exemplos:"
    echo "  $0 patch  # 1.1.3 → 1.1.4"
    echo "  $0 minor  # 1.1.3 → 1.2.0"
    echo "  $0 major  # 1.1.3 → 2.0.0"
    exit 1
fi

# ── Pré-verificações ──────────────────────────────────────────────────────────
BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$BRANCH" != "main" ]; then
    echo "❌ Você precisa estar na branch main (atual: $BRANCH)"
    exit 1
fi

if ! git diff --quiet || ! git diff --staged --quiet; then
    echo "❌ Há alterações não commitadas. Faça commit antes de bumpar."
    exit 1
fi

# ── Lê versão atual ───────────────────────────────────────────────────────────
CARGO_TOML="rust/Cargo.toml"
CURRENT_VERSION=$(grep '^version = ' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/')

if [[ ! "$CURRENT_VERSION" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    echo "❌ Versão inválida no Cargo.toml: $CURRENT_VERSION"
    exit 1
fi

MAJOR="${BASH_REMATCH[1]}"
MINOR="${BASH_REMATCH[2]}"
PATCH="${BASH_REMATCH[3]}"

# ── Calcula nova versão ───────────────────────────────────────────────────────
case "$LEVEL" in
    major)
        NEW_MAJOR=$((MAJOR + 1))
        NEW_MINOR=0
        NEW_PATCH=0
        ;;
    minor)
        NEW_MAJOR=$MAJOR
        NEW_MINOR=$((MINOR + 1))
        NEW_PATCH=0
        ;;
    patch)
        NEW_MAJOR=$MAJOR
        NEW_MINOR=$MINOR
        NEW_PATCH=$((PATCH + 1))
        ;;
esac

NEW_VERSION="${NEW_MAJOR}.${NEW_MINOR}.${NEW_PATCH}"
TAG="v${NEW_VERSION}"

echo "📦 Bump: $CURRENT_VERSION → $NEW_VERSION ($LEVEL)"

# ── Atualiza Cargo.toml ───────────────────────────────────────────────────────
python3 - <<PYEOF
with open('$CARGO_TOML') as f:
    content = f.read()
updated = content.replace('version = "$CURRENT_VERSION"', 'version = "$NEW_VERSION"', 1)
with open('$CARGO_TOML', 'w') as f:
    f.write(updated)
PYEOF

echo "  ✅ Cargo.toml atualizado"

# ── Atualiza Cargo.lock ───────────────────────────────────────────────────────
(cd rust && cargo check --quiet > /dev/null 2>&1)
echo "  ✅ Cargo.lock gerado"

# ── Gera changelog desde a última tag ─────────────────────────────────────────
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

echo ""
echo "📝 Changelog:"
printf '%s\n' "$CHANGELOG" | sed 's/^/   /'

# ── Atualiza README.md ─────────────────────────────────────────────────────────
echo ""
echo "📝 Atualizando README.md..."

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
    r"## What's New — v[\d.]+\n\n.*?\n---",
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
    print("  ✅ README.md atualizado.")
PYEOF

rm -f "$CHANGELOG_TMP"

# ── Diff para review ───────────────────────────────────────────────────────────
echo ""
echo "📋 Diff:"
git diff rust/Cargo.toml README.md | head -50

# ── Commit e tag ──────────────────────────────────────────────────────────────
echo ""
echo "Próximos passos:"
echo "  git add rust/Cargo.toml rust/Cargo.lock README.md"
echo "  git commit -m \"chore: bump version to ${TAG}\""
echo "  git tag -a ${TAG} -m \"Release ${TAG}\""
echo "  git push origin main --follow-tags"
echo ""
read -p "Deseja fazer commit e tag agora? [y/N] " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    git add rust/Cargo.toml rust/Cargo.lock README.md
    git commit -m "chore: bump version to ${TAG}"
    git tag -a "$TAG" -m "Release ${TAG}"
    echo "  ✅ Commit criado"
    echo "  ✅ Tag $TAG criada"
    
    read -p "Deseja fazer push agora? [y/N] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        git push origin main --follow-tags
        echo "  ✅ Push realizado!"
        echo ""
        echo "✅ Release $TAG publicada!"
        echo "   CI: https://github.com/nextlw/elai-code/actions"
    else
        echo "ℹ️  Lembre-se de fazer push manualmente: git push origin main --follow-tags"
    fi
else
    echo "ℹ️  Execute os comandos manualmente acima quando pronto."
fi
