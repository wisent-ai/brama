#!/bin/sh
# Report this host's Brama subscription bundles as JSON: revision and tags.
#
# The renewal loop needs three facts this host alone holds. The `brama:login:`
# tag says which account a subscription signs in with, the item revision says
# whether a login actually wrote a new credential - a login that reports success
# but leaves the bundle at the revision it started from renewed nothing - and the
# vault's trash says which accounts still have a subscription in service at all.
# Retagging never advances a revision, so a revision that moved is a payload that
# was rewritten.
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

# `--all` includes the trash, reported apart from the live bundles. A login item
# whose only subscription bundle was trashed is still an account this fleet
# holds, and the loop has to tell that account from one no bundle has ever been
# attributed to: without the trash, every claude sign-in would read as ambiguous
# and no mapping would ever be learned. A trashed bundle serves no traffic, so it
# is never a renewal candidate and never mixed in with the live ones.
"$ROUTER" list --all | /usr/bin/python3 -c '
import json, os, sys

# One JSON object, so the caller parses an answer rather than scraping a report.
rows = json.load(sys.stdin)
items = {}
trashed = {}
for row in rows:
    identifier = row.get("id") or ""
    if not identifier.startswith("provider:"):
        continue
    where = trashed if row.get("deleted") else items
    where[identifier] = {
        "revision": row.get("revision"),
        "tags": row.get("tags") or [],
        "updated_at": row.get("updated_at"),
    }
json.dump(
    {"vault": os.environ["SKARBIEC_VAULT_FILE"], "items": items, "trashed": trashed},
    sys.stdout,
)
sys.stdout.write("\n")
'
