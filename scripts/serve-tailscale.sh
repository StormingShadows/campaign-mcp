#!/usr/bin/env bash
# Serves Chronicle RPG MCP Server (Streamable HTTP + OAuth 2.1 + Web GM Studio)
# locally over a permanent HTTPS Tailscale Funnel URL on Linux/macOS.
#
# Usage:
#   ./scripts/serve-tailscale.sh [CAMPAIGN_DIR] [PORT]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$REPO_ROOT/.chronicle-serve.env"
[ -f "$ENV_FILE" ] && source "$ENV_FILE"

CAMPAIGN_DIR="${1:-${CHRONICLE_CAMPAIGN_DIR:-$REPO_ROOT/my-campaign}}"
PORT="${2:-${CHRONICLE_PORT:-7878}}"
OAUTH_SECRET="${CHRONICLE_OAUTH_SECRET:-gm-$(openssl rand -base64 18 | tr '+/' '-_' | tr -d '=')}"

if ! command -v tailscale >/dev/null 2>&1; then
  echo "Tailscale is not installed. See https://tailscale.com/download" >&2
  exit 1
fi

DNS_NAME="$(tailscale status --json | python3 -c "import json,sys; print(json.load(sys.stdin)['Self']['DNSName'].rstrip('.'))")"
PUBLIC_URL="https://${DNS_NAME}"
BIN="$REPO_ROOT/target/release/chronicle"

if [ ! -x "$BIN" ]; then
  echo "Building Chronicle release binary..."
  (cd "$REPO_ROOT" && cargo build --release)
fi

if [ ! -f "$ENV_FILE" ]; then
  cat > "$ENV_FILE" <<EOF
CHRONICLE_CAMPAIGN_DIR="$CAMPAIGN_DIR"
CHRONICLE_OAUTH_SECRET="$OAUTH_SECRET"
CHRONICLE_PORT="$PORT"
EOF
fi

cleanup() {
  echo ""
  echo "[chronicle-serve] Disabling Tailscale Funnel on port 443..."
  tailscale funnel --https=443 off >/dev/null 2>&1 || true
  if [ -f "$CAMPAIGN_DIR/campaign.db-wal" ] && command -v python3 >/dev/null 2>&1; then
    python3 -c "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute('PRAGMA wal_checkpoint(TRUNCATE);'); c.close()" "$CAMPAIGN_DIR/campaign.db" || true
  fi
}
trap cleanup EXIT INT TERM

tailscale funnel --bg "$PORT"

echo "============================================================================"
echo "  Permanent MCP URL   : ${PUBLIC_URL}/mcp"
echo "  Web GM Studio GUI   : ${PUBLIC_URL}/"
echo "  GM OAuth Passphrase : ${OAUTH_SECRET}"
echo "  Campaign Folder     : ${CAMPAIGN_DIR}"
echo "============================================================================"

exec "$BIN" mcp \
  --campaign "$CAMPAIGN_DIR" \
  --transport http \
  --host 127.0.0.1 \
  --port "$PORT" \
  --oauth \
  --oauth-secret "$OAUTH_SECRET" \
  --public-url "$PUBLIC_URL"
