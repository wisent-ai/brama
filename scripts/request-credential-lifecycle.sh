#!/usr/bin/env bash
# Ask Weles to act on one provider credential, through its own API.
#
# Codex could be repaired with a local token exchange because its CLI and
# session file sit on this host. Claude and Kimi have neither, and their
# credentials are refreshed by Weles driving a real sign-in. Weles exposes that
# as a credential operation on its loopback API, which is the sanctioned way to
# reach it -- not by running a trajectory script by hand.
#
# Defaults to `verify`, which inspects a credential and changes nothing. The
# operation and provider come from the environment so this file states no
# irreversible action of its own:
#
#   LIFECYCLE_OPERATION=verify|acquire|adopt|rotate|remove|reset
#   LIFECYCLE_PROVIDER=claude_code|kimi|codex|...
#   LIFECYCLE_DRY_RUN=true|false
#
# Prints the API's answer. The token is read from the vault and never printed.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

OPERATION="${LIFECYCLE_OPERATION:-verify}"
PROVIDER="${LIFECYCLE_PROVIDER:-claude_code}"
DRY_RUN="${LIFECYCLE_DRY_RUN:-true}"
ENDPOINT="${LIFECYCLE_ENDPOINT:-http://127.0.0.1:8794/v1/echo/secrets/acquire}"
SKARBIEC="$HOME/.stado/bin/skarbiec"

[ -x "$SKARBIEC" ] || { echo "no skarbiec at $SKARBIEC" >&2; exit 1; }

token=$(SKARBIEC_VAULT_FILE="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}" \
  "$SKARBIEC" get echo-weles-api 2>/dev/null | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["fields"]["token"])' 2>/dev/null || true)
if [ -z "${token:-}" ]; then
  echo "cannot read echo-weles-api/token from this vault" >&2
  exit 1
fi

echo "endpoint:  $ENDPOINT"
echo "operation: $OPERATION   provider: $PROVIDER   dry_run: $DRY_RUN"
echo "--- answer ---"
/usr/bin/curl -s -m 120 -X POST "$ENDPOINT" \
  -H "Authorization: Bearer ${token}" \
  -H 'Content-Type: application/json' \
  -d "{\"version\":\"skarbiec.credential-operation.v3\",\"operation\":\"${OPERATION}\",\"provider\":\"${PROVIDER}\",\"dry_run\":${DRY_RUN}}" \
  -w '\nHTTP %{http_code}\n' | /usr/bin/head -c 1200
echo
