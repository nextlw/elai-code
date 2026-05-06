#!/usr/bin/env bash
# Serve a documentação interativa da API do elai-server localmente.
#
# Uso:
#   ./serve.sh           # porta 8080
#   ./serve.sh 9090      # porta customizada
#
set -euo pipefail
PORT="${1:-8080}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/site" && pwd)"

echo "📘 elai-server API docs"
echo "   diretório: $DIR"
echo "   abrindo:   http://127.0.0.1:${PORT}/"
echo

if command -v python3 >/dev/null 2>&1; then
  cd "$DIR"
  exec python3 -m http.server "$PORT" --bind 127.0.0.1
elif command -v npx >/dev/null 2>&1; then
  cd "$DIR"
  exec npx --yes http-server -p "$PORT" -a 127.0.0.1 -c-1 .
else
  echo "✖ nem python3 nem npx encontrados. Instale um deles para servir os docs." >&2
  exit 1
fi
