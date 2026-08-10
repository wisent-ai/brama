#!/bin/sh
# Report this host's Brama subscription material: which vault items exist, what
# tags they carry, and which credential sources are present. Metadata only -
# item ids, kinds and tags, never a field value.
set -eu

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

BUNDLE="$HOME/.stado/services/brama/current/darwin-arm"
ROUTER="$BUNDLE/bin/skarbiec-entitlements-router"
[ -x "$ROUTER" ] || { printf 'no entitlements router at %s\n' "$ROUTER"; exit 1; }

SERVICE_ENV="${BRAMA_SERVICE_ENV_FILE:-$HOME/.config/brama/service.env}"
if [ -f "$SERVICE_ENV" ]; then
  for key in SKARBIEC_VAULT_FILE BRAMA_SKARBIEC_CONFIG_DIR; do
    value=$(sed -n "s/^${key}=//p" "$SERVICE_ENV" | tail -1 | tr -d '"')
    [ -n "$value" ] && export "$key=$value"
  done
fi

printf 'service env: %s (present: %s)\n' "$SERVICE_ENV" "$([ -f "$SERVICE_ENV" ] && echo yes || echo no)"
printf 'vault file: %s\n' "${SKARBIEC_VAULT_FILE:-(router default)}"
printf 'catalog file: %s\n' "$BUNDLE/etc/brama-skarbiec/subscriptions.json"

"$ROUTER" list | /usr/bin/python3 -c '
import json, sys
rows = [row for row in json.load(sys.stdin) if not row.get("deleted")]
print("items total:", len(rows))
for row in rows:
    identifier = row.get("id") or ""
    tags = row.get("tags") or []
    if identifier.startswith("provider:") or "reauth-config" in identifier:
        print(" ", identifier, "| tags:", ",".join(tags) or "(none)")
'
