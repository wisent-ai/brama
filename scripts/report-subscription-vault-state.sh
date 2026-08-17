#!/bin/sh
# Report this host's Brama subscription bundles as JSON: revision and tags.
#
# The renewal loop needs two facts this host alone holds. The `brama:login:`
# tag says which account a subscription signs in with, and the item revision
# says whether a login actually wrote a new credential: a login that reports
# success but leaves the bundle at the revision it started from renewed
# nothing. Retagging never advances a revision, so a revision that moved is a
# payload that was rewritten.
#
# Metadata only - item ids, revisions and tags, never a field value. Read-only:
# nothing here writes to the vault. Installed and run through Stado:
#   stado host install-helper <host> scripts/report-subscription-vault-state.sh \
#     report-subscription-vault-state
#   stado host run-helper <host> report-subscription-vault-state
set -eu

# `run-helper` hands over a minimal environment, and the vault is PGP-encrypted,
# so the tools skarbiec spawns have to be reachable or a present vault reads as
# an absent one.
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

BUNDLE="$HOME/.stado/services/brama/current/darwin-arm"
ROUTER=${SKARBIEC_BIN:-"$BUNDLE/bin/skarbiec-entitlements-router"}
[ -x "$ROUTER" ] || ROUTER="$HOME/.stado/bin/skarbiec"
[ -x "$ROUTER" ] || {
  printf 'no skarbiec binary on this host: looked at %s/bin/skarbiec-entitlements-router and %s/.stado/bin/skarbiec\n' \
    "$BUNDLE" "$HOME" >&2
  exit 1
}

# The gateway's own vault, taken from the service environment when it names one,
# because reporting revisions from a different vault than the one the gateway
# reads would make the renewal loop verify a file nobody serves from.
SERVICE_ENV=${BRAMA_SERVICE_ENV_FILE:-"$HOME/.config/brama/service.env"}
if [ -z "${SKARBIEC_VAULT_FILE:-}" ] && [ -f "$SERVICE_ENV" ]; then
  value=$(sed -n 's/^SKARBIEC_VAULT_FILE=//p' "$SERVICE_ENV" | tail -1 | tr -d '"')
  [ -n "$value" ] && SKARBIEC_VAULT_FILE=$value
fi
SKARBIEC_VAULT_FILE=${SKARBIEC_VAULT_FILE:-"$HOME/.stado/skarbiec.vault.json"}
[ -f "$SKARBIEC_VAULT_FILE" ] || {
  printf 'no fleet vault at %s\n' "$SKARBIEC_VAULT_FILE" >&2
  exit 1
}
export SKARBIEC_VAULT_FILE

"$ROUTER" list | /usr/bin/python3 -c '
import json, os, sys

# One JSON object, so the caller parses an answer rather than scraping a report.
rows = json.load(sys.stdin)
items = {}
for row in rows:
    identifier = row.get("id") or ""
    if row.get("deleted") or not identifier.startswith("provider:"):
        continue
    items[identifier] = {
        "revision": row.get("revision"),
        "tags": row.get("tags") or [],
        "updated_at": row.get("updated_at"),
    }
json.dump({"vault": os.environ["SKARBIEC_VAULT_FILE"], "items": items}, sys.stdout)
sys.stdout.write("\n")
'
