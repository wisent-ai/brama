#!/bin/sh
# Run the subscription renewal loop for as long as this host is up.
#
# The loop itself is one sweep: it reads the gateway's own subscription listing,
# picks the rows whose newest plan-usage probe was refused for an authentication
# reason, signs the mapped account in through Weles, and proves the repair by an
# advanced vault revision plus a probe that now succeeds. What was missing was
# anything that runs it. A credential that expires at three in the morning has to
# be replaced without a person, and the evidence that this was ever the plan is
# five days of refused subscriptions that nobody noticed until an operator looked
# at a screen.
#
# Stado renders long-running units, not timers, so the schedule lives here: one
# sweep, then sleep. The sweep is cheap when there is nothing to do, and the
# loop's own cooldown - not this interval - decides how often one account is
# actually signed in again, so running often is safe.
#
# Secrets never reach a command line. The bearer and the agent's request-signing
# secret are read from their exact Skarbiec items through the local entitlements
# router, exactly as Brama's own launcher reads them, and handed to the sweep on
# standard input.
set -eu

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

SWEEP="${BRAMA_RENEWAL_SWEEP_COMMAND:-$HOME/.stado/bin/renew-refused-subscriptions}"
ROUTER="${BRAMA_RENEWAL_ROUTER_BIN:-$HOME/.stado/services/brama/current/darwin-arm/bin/skarbiec-entitlements-router}"
ORIGIN="${BRAMA_RENEWAL_ORIGIN:-http://127.0.0.1:8080}"
AGENT="${BRAMA_RENEWAL_AGENT:-wisent-app}"
BEARER_ITEM="${BRAMA_RENEWAL_BEARER_ITEM:-wisent-app-model-router}"
BEARER_FIELD="${BRAMA_RENEWAL_BEARER_FIELD:-token}"
SECRET_ITEM="${BRAMA_RENEWAL_SECRET_ITEM:-agent:wisent-app}"
SECRET_FIELD="${BRAMA_RENEWAL_SECRET_FIELD:-value}"
INTERVAL="${BRAMA_RENEWAL_SWEEP_SECONDS:-3600}"
FIRST_DELAY="${BRAMA_RENEWAL_FIRST_SWEEP_DELAY_SECONDS:-120}"
SKARBIEC_VAULT_FILE="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
export SKARBIEC_VAULT_FILE

[ -x "$SWEEP" ] || {
  printf 'no renewal sweep at %s; install it as a host helper first\n' "$SWEEP" >&2
  exit 1
}
[ -x "$ROUTER" ] || {
  printf 'no entitlements router at %s; this host cannot read the vault items the sweep needs\n' "$ROUTER" >&2
  exit 1
}

# One field of one item, printed by the router and parsed here rather than by a
# shell pattern, because a credential that silently becomes an empty string turns
# a signed request into an unauthenticated one and the gateway then answers 401
# about a token nobody chose.
read_field() {
  item=$1
  name=$2
  "$ROUTER" get "$item" 2>/dev/null | ITEM="$item" FIELD="$name" /usr/bin/python3 -c '
import json, os, sys

item = os.environ["ITEM"]
field = os.environ["FIELD"]
try:
    payload = json.load(sys.stdin)
except ValueError:
    sys.stderr.write(f"{item} did not return a JSON item\n")
    raise SystemExit(1)
fields = payload.get("fields")
if not isinstance(fields, dict):
    sys.stderr.write(f"{item} returned no fields object\n")
    raise SystemExit(1)
value = fields.get(field)
if not isinstance(value, str) or not value.strip():
    sys.stderr.write(f"{item}/{field} is empty\n")
    raise SystemExit(1)
sys.stdout.write(value.strip())
'
}

/bin/sleep "$FIRST_DELAY"

while :; do
  started=$(/bin/date -u '+%Y-%m-%dT%H:%M:%SZ')
  if bearer=$(read_field "$BEARER_ITEM" "$BEARER_FIELD") \
    && secret=$(read_field "$SECRET_ITEM" "$SECRET_FIELD"); then
    if printf '%s\n%s\n' "$bearer" "$secret" | "$SWEEP" "$ORIGIN" "$AGENT"; then
      printf 'renewal sweep %s: nothing left refused\n' "$started"
    else
      printf 'renewal sweep %s: reported work still open, see the lines above\n' "$started"
    fi
  else
    printf 'renewal sweep %s: could not read the credentials the sweep needs\n' "$started" >&2
  fi
  unset bearer secret
  /bin/sleep "$INTERVAL"
done
