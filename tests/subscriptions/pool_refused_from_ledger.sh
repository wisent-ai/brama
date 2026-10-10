#!/usr/bin/env bash
# Real test of the `best` selector over a pool the ledger already refuses,
# through the built `brama test`, which runs the same ranked walk the HTTP
# edge runs for a signed agent. The pool is a trusted catalog of two members
# and the ledger a file this run writes; the entitlements router is a path that
# does not exist, so any read of a credential can only fail by naming it.
#
# The story: every member awaiting a sign-in is refused from the ledger before
# any read, with the members and the provider's own refusals named and no
# attempt made; a member the ledger does not refuse is read, and the refusal
# then names the router that could not be reached, which proves the read path
# was taken and the ledger path was not; and the usage refusals the command
# makes before any of that. Every command, its outcome and its answer go to
# the run's report.txt; a failed check stops the run after naming itself there.
#
# Usage: BRAMA=target/release/brama tests/subscriptions/pool_refused_from_ledger.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/subscriptions/pool-refused-from-ledger-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"

CALLER=test-caller
FIRST=member-one
SECOND=member-two
CAUSE_FIRST='the provider refused the refresh: the stored grant is no longer valid'
CAUSE_SECOND='retired for the real test'
export BRAMA_SUBSCRIPTION_CATALOG="{\"items\":[{\"id\":\"$FIRST\",\"provider\":\"claude-code\",\"status\":\"active\"},{\"id\":\"$SECOND\",\"provider\":\"claude-code\",\"status\":\"active\"}]}"
export BRAMA_SUBSCRIPTION_USAGE_FILE="$ROOT/ledger.json"
export ENTITLEMENTS_ROUTER_BIN="$ROOT/no-entitlements-router"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# run accepted|refused ARGS...: the command must end the way named; its whole
# answer, standard output and error together, goes to the report, and the JSON
# document it printed to answer.json beside it.
run() {
  expected=$1
  shift
  if "$BIN" "$@" &>"$ROOT/output" </dev/null; then outcome=accepted; else outcome=refused; fi
  out=$(cat "$ROOT/output")
  sed -n '/^{/,/^}/p' "$ROOT/output" >"$ROOT/answer.json"
  printf '$ brama %s\noutcome: %s\noutput: %s\n\n' "$*" "$outcome" "$out" >>"$REPORT"
  [ "$outcome" = "$expected" ] || fail "$* was $outcome, expected $expected: $out"
}
says() {
  case "$out" in
    *"$1"*) echo "ok: $2" >>"$REPORT" ;;
    *) fail "$2: expected '$1' in: $out" ;;
  esac
}
silent() {
  case "$out" in
    *"$1"*) fail "$2: did not expect '$1' in: $out" ;;
    *) echo "ok: $2" >>"$REPORT" ;;
  esac
}
# field JQ: the answer document's field, or the empty string.
field() {
  jq -r "$1 // empty" "$ROOT/answer.json"
}

# --- the usage refusals, before any ledger or pool is read ---
printf '%s' '{"subscriptions":{}}' >"$BRAMA_SUBSCRIPTION_USAGE_FILE"
run refused test --model best --agent-id "$CALLER"
says "refusing billable inference without explicit --allow-provider-cost" "a billable test without cost acknowledgement is refused"
run refused test --agent-id "$CALLER" --allow-provider-cost
says "--model" "a test without a route is refused by the parser"

# --- every member refused by the ledger: no read, the members and causes named ---
printf '%s' "{\"subscriptions\":{\"$FIRST\":{\"provider\":\"claude-code\",\"credential\":{\"state\":\"needs_reauthorization\",\"cause\":\"$CAUSE_FIRST\"}},\"$SECOND\":{\"provider\":\"claude-code\",\"credential\":{\"state\":\"disabled\",\"cause\":\"$CAUSE_SECOND\"}}}}" >"$BRAMA_SUBSCRIPTION_USAGE_FILE"
asked=$(date -u +%s)
run refused test --model best --agent-id "$CALLER" --allow-provider-cost --json
answered=$(date -u +%s)
echo "seconds from asking to the refusal: $((answered - asked))" >>"$REPORT"
says "every subscription is refused by the ledger before any is read:" "a pool the ledger refuses entirely is refused as such"
says "$FIRST: awaiting sign-in: $CAUSE_FIRST" "the first member is named with the provider's own refusal"
says "$SECOND: awaiting sign-in: $CAUSE_SECOND" "the retired member is named with its recorded cause"
silent "no-entitlements-router" "no credential was read through the router"
[ "$(field .code)" = subscription_reauthorization_required ] ||
  fail "the refusal's code is $(field .code), expected subscription_reauthorization_required"
[ "$(field .attempts)" = "$(jq -n '[] | length')" ] ||
  fail "the refusal cost $(field .attempts) provider attempts; the ledger path makes none"
echo "ok: refused as subscription_reauthorization_required with no provider attempt" >>"$REPORT"

# --- a member the ledger does not refuse is read: the read path, and its refusal names the router ---
printf '%s' "{\"subscriptions\":{\"$FIRST\":{\"provider\":\"claude-code\",\"credential\":{\"state\":\"needs_reauthorization\",\"cause\":\"$CAUSE_FIRST\"}}}}" >"$BRAMA_SUBSCRIPTION_USAGE_FILE"
run refused test --model best --agent-id "$CALLER" --allow-provider-cost --json
says "could not discover native provider models:" "a member the ledger holds nothing against is read"
says "$FIRST: awaiting sign-in: $CAUSE_FIRST" "the refused member is still named beside it"
says "$SECOND:" "the read member's refusal is named"
silent "every subscription is refused by the ledger" "the ledger sentence is not used when a member was read"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
