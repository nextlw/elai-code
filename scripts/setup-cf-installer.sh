#!/usr/bin/env bash
#
# setup-cf-installer.sh
#
# Provisions get.nexcode.live as a Cloudflare Worker that proxies to the
# correct install script (install.sh for unix, install.ps1 for PowerShell)
# based on the client's User-Agent.
#
# Reads credentials from <repo>/.env using the CLOUD_FLARE_* naming.
# Auth uses the Global API Key (X-Auth-Email + X-Auth-Key).
#
# Required env vars (in .env):
#   CLOUD_FLARE_EMAIL
#   CLOUD_FLARE_API_KEY
#   CLOUD_FLARE_ACCOUNT_ID
#   CLOUD_FLARE_ZONE_ID
#   CLOUD_FLARE_ZONE_DOMAIN   (e.g. nexcode.live)
#
# Optional overrides:
#   SUBDOMAIN      (default: get)
#   WORKER_NAME    (default: elai-installer)
#   INSTALL_SH_URL (default: GitHub raw of scripts/install.sh)
#   INSTALL_PS_URL (default: GitHub raw of scripts/install.ps1)

set -euo pipefail

# ---------- locate repo + load .env ----------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ENV_FILE="$REPO_ROOT/.env"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "error: $ENV_FILE not found" >&2
  exit 1
fi

# Source only CLOUD_FLARE_* keys to avoid polluting the shell with the entire .env.
while IFS= read -r line; do
  [[ "$line" =~ ^[[:space:]]*# ]] && continue
  [[ "$line" =~ ^[[:space:]]*$ ]] && continue
  if [[ "$line" =~ ^[[:space:]]*(CLOUD_FLARE_[A-Z_]+)=(.*)$ ]]; then
    key="${BASH_REMATCH[1]}"
    val="${BASH_REMATCH[2]}"
    val="${val%\"}"; val="${val#\"}"
    val="${val%\'}"; val="${val#\'}"
    export "$key=$val"
  fi
done < "$ENV_FILE"

# ---------- validate ----------
require() { [[ -n "${!1:-}" ]] || { echo "error: $1 missing in .env" >&2; exit 1; }; }
require CLOUD_FLARE_EMAIL
require CLOUD_FLARE_API_KEY
require CLOUD_FLARE_ACCOUNT_ID
require CLOUD_FLARE_ZONE_ID
require CLOUD_FLARE_ZONE_DOMAIN

command -v curl >/dev/null || { echo "error: curl required" >&2; exit 1; }
command -v jq   >/dev/null || { echo "error: jq required (brew install jq)" >&2; exit 1; }

# ---------- config ----------
SUBDOMAIN="${SUBDOMAIN:-get}"
HOSTNAME="${SUBDOMAIN}.${CLOUD_FLARE_ZONE_DOMAIN}"
WORKER_NAME="${WORKER_NAME:-elai-installer}"
INSTALL_SH_URL="${INSTALL_SH_URL:-https://raw.githubusercontent.com/nextlw/elai-code/main/scripts/install.sh}"
INSTALL_PS_URL="${INSTALL_PS_URL:-https://raw.githubusercontent.com/nextlw/elai-code/main/scripts/install.ps1}"

API="https://api.cloudflare.com/client/v4"
AUTH_EMAIL_HDR="X-Auth-Email: $CLOUD_FLARE_EMAIL"
AUTH_KEY_HDR="X-Auth-Key: $CLOUD_FLARE_API_KEY"

cf() {
  curl -fsS \
    -H "$AUTH_EMAIL_HDR" \
    -H "$AUTH_KEY_HDR" \
    -H "Content-Type: application/json" \
    "$@"
}

show_status() {
  local resp="$1" label="$2"
  if echo "$resp" | jq -e '.success == true' >/dev/null 2>&1; then
    echo "  ✓ $label"
  else
    echo "  ✗ $label"
    echo "$resp" | jq '.errors // .' >&2
    exit 1
  fi
}

echo "==> Target: https://$HOSTNAME"
echo "    Worker: $WORKER_NAME"
echo "    Account: ${CLOUD_FLARE_ACCOUNT_ID:0:8}…  Zone: ${CLOUD_FLARE_ZONE_ID:0:8}…"

# ---------- 1. build worker source ----------
WORKER_FILE="$(mktemp -t elai-worker.XXXXXX).js"
trap 'rm -f "$WORKER_FILE"' EXIT

cat > "$WORKER_FILE" <<EOF
export default {
  async fetch(req, env, ctx) {
    const t0 = Date.now();
    const ua = (req.headers.get('user-agent') || '').toLowerCase();
    const url = new URL(req.url);
    const forcePS = url.pathname === '/ps' || url.pathname === '/install.ps1';
    const forceSH = url.pathname === '/sh' || url.pathname === '/install.sh';
    const isPS = forcePS || (!forceSH && /powershell|windowspowershell/.test(ua));

    const target = isPS
      ? '${INSTALL_PS_URL}'
      : '${INSTALL_SH_URL}';

    let upstreamStatus = 0;
    let upstreamMs = 0;
    let response;
    try {
      const u0 = Date.now();
      const upstream = await fetch(target, { cf: { cacheTtl: 300, cacheEverything: true } });
      upstreamMs = Date.now() - u0;
      upstreamStatus = upstream.status;
      if (!upstream.ok) {
        response = new Response('install script unavailable', { status: 502 });
      } else {
        response = new Response(upstream.body, {
          status: 200,
          headers: {
            'content-type': isPS ? 'text/plain; charset=utf-8' : 'text/x-shellscript; charset=utf-8',
            'cache-control': 'public, max-age=300',
            'x-elai-target': isPS ? 'ps1' : 'sh',
          },
        });
      }
    } catch (err) {
      response = new Response('install script error', { status: 502 });
      upstreamStatus = -1;
      console.error(JSON.stringify({ event: 'upstream_error', target, error: String(err) }));
    }

    ctx.waitUntil(Promise.resolve().then(() => {
      console.log(JSON.stringify({
        event: 'install_request',
        target: isPS ? 'ps1' : 'sh',
        path: url.pathname,
        forced: forcePS ? 'ps' : (forceSH ? 'sh' : null),
        status: response.status,
        upstream_status: upstreamStatus,
        upstream_ms: upstreamMs,
        total_ms: Date.now() - t0,
        ua: ua.slice(0, 160),
        country: req.cf && req.cf.country,
        colo: req.cf && req.cf.colo,
        asn: req.cf && req.cf.asn,
      }));
    }));

    return response;
  }
};
EOF

# ---------- 2. upload worker ----------
echo "==> Uploading Worker..."
UPLOAD_RESP=$(curl -sS \
  -X PUT \
  -H "$AUTH_EMAIL_HDR" \
  -H "$AUTH_KEY_HDR" \
  "$API/accounts/$CLOUD_FLARE_ACCOUNT_ID/workers/scripts/$WORKER_NAME" \
  -F 'metadata={"main_module":"worker.js","compatibility_date":"2025-01-01"};type=application/json' \
  -F "worker.js=@$WORKER_FILE;filename=worker.js;type=application/javascript+module")
show_status "$UPLOAD_RESP" "Worker '$WORKER_NAME' uploaded"

# ---------- 3. enable workers.dev subdomain (idempotent) ----------
cf -X POST "$API/accounts/$CLOUD_FLARE_ACCOUNT_ID/workers/scripts/$WORKER_NAME/subdomain" \
  -d '{"enabled":false}' >/dev/null 2>&1 || true

# ---------- 4. bind custom domain ----------
echo "==> Binding $HOSTNAME -> $WORKER_NAME"
DOMAIN_PAYLOAD=$(jq -n \
  --arg env  "production" \
  --arg host "$HOSTNAME" \
  --arg svc  "$WORKER_NAME" \
  --arg zone "$CLOUD_FLARE_ZONE_ID" \
  '{environment:$env, hostname:$host, service:$svc, zone_id:$zone}')

DOMAIN_RESP=$(cf -X PUT "$API/accounts/$CLOUD_FLARE_ACCOUNT_ID/workers/domains" \
  -d "$DOMAIN_PAYLOAD")
show_status "$DOMAIN_RESP" "Custom domain attached"

# ---------- 5. smoke test ----------
echo ""
echo "==> Setup complete. Verify:"
echo "    curl -fsSL https://$HOSTNAME | head -20"
echo "    curl -fsSL https://$HOSTNAME/ps | head -20    # force PowerShell"
echo ""
echo "Then update README install commands to:"
echo "    curl -fsSL https://$HOSTNAME | sh"
echo "    irm https://$HOSTNAME/ps | iex"
