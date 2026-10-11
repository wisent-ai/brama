#!/usr/bin/env bash
# Real test of the pool holding only real accounts' subscriptions, through
# `brama subscription list --apply` (the same operation as POST
# /v1/subscription-pool) on the built binary and a real Skarbiec vault this
# run creates and owns. The vault holds one subscription item that names no
# account.
#
# The story: banking a new member with no account is refused; re-banking the
# vault's accountless member without naming an account is refused with the
# sentence that the pool holds only real accounts' subscriptions, and the
# vault item is left as it was; re-banking it naming the account is accepted
# and the pool then lists the member with that account. Every command, its
# outcome and its answer go to the run's report.txt; a failed check stops the
# run after naming itself there.
#
# Usage: BRAMA=target/release/brama SKARBIEC_BIN=$(command -v skarbiec) BANK_ACCOUNT=<address the run's member is banked under> tests/subscriptions/accounts/bank_requires_account.sh
set -eu
cd "$(dirname "$0")/../../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
VAULT_BIN=${SKARBIEC_BIN:?set SKARBIEC_BIN to the skarbiec binary that owns this run's vault}
ACCOUNT=${BANK_ACCOUNT:?set BANK_ACCOUNT to the account address this run banks its vault member under}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/subscriptions/bank-requires-account-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT/state" "$ROOT/keyring"
chmod 700 "$ROOT/keyring"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
echo "vault binary: $VAULT_BIN" >>"$REPORT"

MEMBER=member-without-account
export BRAMA_STATE_DIR="$ROOT/state"
export BRAMA_SUBSCRIPTION_USAGE_FILE="$ROOT/ledger.json"
export GNUPGHOME="$ROOT/keyring"
export SKARBIEC_VAULT_FILE="$ROOT/vault.json"
export SKARBIEC_AUDIT_FILE="$ROOT/audit.jsonl"
export ENTITLEMENTS_ROUTER_BIN="$VAULT_BIN"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# vault ARGS...: one real Skarbiec command on this run's vault; its answer
# goes to the report and a refusal stops the run.
vault() {
  if "$VAULT_BIN" "$@" &>"$ROOT/vault-output" </dev/null; then outcome=accepted; else outcome=refused; fi
  printf '$ skarbiec %s\noutcome: %s\noutput: %s\n\n' "$*" "$outcome" "$(cat "$ROOT/vault-output")" >>"$REPORT"
  [ "$outcome" = accepted ] || fail "skarbiec $* was refused: $(cat "$ROOT/vault-output")"
}
# apply accepted|refused DOCUMENT: the membership document on standard input
# must end the way named; the whole answer goes to the report.
apply() {
  expected=$1
  if printf '%s' "$2" | "$BIN" subscription list --apply --json &>"$ROOT/output"; then outcome=accepted; else outcome=refused; fi
  out=$(cat "$ROOT/output")
  printf '$ brama subscription list --apply --json <<< %s\noutcome: %s\noutput: %s\n\n' \
    "$(printf '%s' "$2" | sed 's/"api_key":"[^"]*"/"api_key":"<redacted>"/')" "$outcome" "$out" >>"$REPORT"
  [ "$outcome" = "$expected" ] || fail "apply was $outcome, expected $expected: $out"
}
says() {
  case "$out" in
    *"$1"*) echo "ok: $2" >>"$REPORT" ;;
    *) fail "$2: expected '$1' in: $out" ;;
  esac
}
vault_item() {
  "$VAULT_BIN" get-json "$MEMBER" </dev/null 2>/dev/null
}

# --- a real vault with one subscription item that names no account ---
vault init bank-journey-owner
printf '%s' '{"token":"not-a-real-grant"}' | "$VAULT_BIN" set-json "$MEMBER" --type token --tags "brama:subscription,brama:provider:claude-code,brama:id:$MEMBER" &>"$ROOT/vault-output" ||
  fail "skarbiec set-json $MEMBER was refused: $(cat "$ROOT/vault-output")"
printf '$ skarbiec set-json %s --type token --tags brama:subscription,brama:provider:claude-code,brama:id:%s\noutput: %s\n\n' "$MEMBER" "$MEMBER" "$(cat "$ROOT/vault-output")" >>"$REPORT"
before=$(vault_item)

# --- refusals: no account anywhere ---
apply refused '{"action":"bank","agent_id":"journey","provider":"claude-code","api_key":"not-a-real-grant"}'
says "account email address is required when adding a subscription" "a new member with no account is refused"
apply refused "{\"action\":\"bank\",\"agent_id\":\"journey\",\"provider\":\"claude-code\",\"api_key\":\"not-a-real-grant\",\"subscription_id\":\"$MEMBER\"}"
says "the selected subscription names no account, and the pool holds only real accounts' subscriptions" \
  "re-banking an accountless member without an account is refused"
[ "$(vault_item)" = "$before" ] || fail "a refused bank changed the vault item"
echo "ok: the refused bank left the vault item as it was" >>"$REPORT"

# --- naming the real account is accepted, and the pool lists it with that account ---
apply accepted "{\"action\":\"bank\",\"agent_id\":\"journey\",\"provider\":\"claude-code\",\"api_key\":\"not-a-real-grant\",\"subscription_id\":\"$MEMBER\",\"account\":\"$ACCOUNT\"}"
if "$BIN" subscription list --json &>"$ROOT/output" </dev/null; then :; fi
out=$(cat "$ROOT/output")
printf '$ brama subscription list --json\noutput: %s\n\n' "$out" >>"$REPORT"
says "$MEMBER" "the re-banked member is listed"
says "$ACCOUNT" "and carries the account it was banked with"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
