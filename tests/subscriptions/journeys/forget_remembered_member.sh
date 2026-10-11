#!/usr/bin/env bash
# Real test of `brama subscription forget` on this host's own pool, through
# the built binary and a real Skarbiec vault this run creates and owns. The
# vault holds one subscription item; the usage ledger is a file this run
# writes, holding that member and one more the vault does not list; the
# journal and lease register live in a state directory this run owns.
#
# The story: `list` names only the member the vault holds — the one the
# ledger alone remembers is no pool member; forgetting it with a reason is
# accepted, the journal gains a `forgotten` record carrying that reason, the
# ledger loses the row, and the next `list` still does not name it; forgetting the listed
# member is refused back to `disown`; forgetting an id nobody knows is
# refused as such; an empty reason is refused by the parser. Every command,
# its outcome and its answer go to the run's report.txt; a failed check stops
# the run after naming itself there.
#
# Usage: BRAMA=target/release/brama SKARBIEC_BIN=$(command -v skarbiec) tests/subscriptions/journeys/forget_remembered_member.sh
set -eu
cd "$(dirname "$0")/../../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
VAULT_BIN=${SKARBIEC_BIN:?set SKARBIEC_BIN to the skarbiec binary that owns this run's vault}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/subscriptions/forget-remembered-member-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT/state" "$ROOT/keyring"
chmod 700 "$ROOT/keyring"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
echo "vault binary: $VAULT_BIN" >>"$REPORT"

LISTED=member-listed
REMEMBERED=member-remembered
UNKNOWN=member-unknown
REASON="this account was never signed in on this gateway"
export BRAMA_STATE_DIR="$ROOT/state"
export BRAMA_SUBSCRIPTION_USAGE_FILE="$ROOT/ledger.json"
export GNUPGHOME="$ROOT/keyring"
export SKARBIEC_VAULT_FILE="$ROOT/vault.json"
export SKARBIEC_AUDIT_FILE="$ROOT/audit.jsonl"
export ENTITLEMENTS_ROUTER_BIN="$VAULT_BIN"

# vault ARGS...: one real Skarbiec command on this run's vault; its answer
# goes to the report and a refusal stops the run.
vault() {
  if "$VAULT_BIN" "$@" &>"$ROOT/vault-output" </dev/null; then outcome=accepted; else outcome=refused; fi
  printf '$ skarbiec %s\noutcome: %s\noutput: %s\n\n' "$*" "$outcome" "$(cat "$ROOT/vault-output")" >>"$REPORT"
  [ "$outcome" = accepted ] || fail "skarbiec $* was refused: $(cat "$ROOT/vault-output")"
}

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# run accepted|refused|either ARGS...: the command must end the way named
# (`either` reads its answer only: `list` exits non-zero whenever any row of
# the report carries an error, which is the state under test); its whole
# answer, standard output and error together, goes to the report, and the JSON
# document it printed to answer.json beside it.
run() {
  expected=$1
  shift
  if "$BIN" "$@" &>"$ROOT/output" </dev/null; then outcome=accepted; else outcome=refused; fi
  out=$(cat "$ROOT/output")
  sed -n '/^{/,/^}/p' "$ROOT/output" >"$ROOT/answer.json"
  printf '$ brama %s\noutcome: %s\noutput: %s\n\n' "$*" "$outcome" "$out" >>"$REPORT"
  [ "$expected" = either ] || [ "$outcome" = "$expected" ] || fail "$* was $outcome, expected $expected: $out"
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

# --- a real vault with one subscription item; the ledger remembers that member and one more ---
vault init forget-journey-owner
printf '%s' '{"token":"not-a-real-grant"}' | "$VAULT_BIN" set-json "$LISTED" --type token --tags "brama:subscription,brama:provider:claude-code,brama:id:$LISTED" &>"$ROOT/vault-output" ||
  fail "skarbiec set-json $LISTED was refused: $(cat "$ROOT/vault-output")"
printf '$ skarbiec set-json %s --type token --tags brama:subscription,brama:provider:claude-code,brama:id:%s\noutput: %s\n\n' "$LISTED" "$LISTED" "$(cat "$ROOT/vault-output")" >>"$REPORT"
printf '%s' "{\"subscriptions\":{\"$LISTED\":{\"provider\":\"claude-code\"},\"$REMEMBERED\":{\"provider\":\"claude-code\"}}}" >"$BRAMA_SUBSCRIPTION_USAGE_FILE"
run either subscription list --json
[ "$(jq -r "[.subscriptions[].id] | index(\"$REMEMBERED\")" "$ROOT/answer.json")" = null ] ||
  fail "a member only the usage history remembers is listed among the pool's members"
[ "$(jq -r "[.remembered[].id] | index(\"$REMEMBERED\")" "$ROOT/answer.json")" != null ] ||
  fail "the remembered id is not answered under remembered: $(cat "$ROOT/answer.json")"
[ "$(jq -r "[.subscriptions[].id] | index(\"$LISTED\")" "$ROOT/answer.json")" != null ] ||
  fail "the member the vault holds is not listed"
echo "ok: the pool lists the vault's member and answers the remembered id apart" >>"$REPORT"
run either subscription list
says "remembered by the usage history only, not pool members" "the text names the remembered ids apart"

# --- the refusals, each with its own cause ---
run refused subscription forget --subscription-id "$REMEMBERED" --reason ""
says "--reason" "an empty reason is refused by the parser"
run refused subscription forget --subscription-id "$LISTED" --reason "$REASON"
says "the vault lists \`$LISTED\` (claude-code) as a member of the pool" "a member the vault lists is refused"
says "brama subscription disown" "and sent to disown"
run refused subscription forget --subscription-id "$UNKNOWN" --reason "$REASON"
says "neither the vault nor the usage history knows \`$UNKNOWN\`" "an id nobody knows is refused as such"
[ ! -e "$BRAMA_STATE_DIR/journal.jsonl" ] || fail "a refusal wrote the journal: $(cat "$BRAMA_STATE_DIR/journal.jsonl")"
echo "ok: no refusal wrote the journal" >>"$REPORT"

# --- the remembered member is forgotten with the reason ---
run accepted subscription forget --subscription-id "$REMEMBERED" --reason "$REASON" --json
[ "$(jq -r '.subscription_id' "$ROOT/answer.json")" = "$REMEMBERED" ] ||
  fail "the answer names $(jq -r '.subscription_id' "$ROOT/answer.json"), expected $REMEMBERED"
says "existed only in the usage history and is forgotten" "the answer says what happened"
record=$(jq -c "select(.kind == \"forgotten\" and .id == \"$REMEMBERED\")" "$BRAMA_STATE_DIR/journal.jsonl")
[ -n "$record" ] || fail "the journal holds no forgotten record for $REMEMBERED: $(cat "$BRAMA_STATE_DIR/journal.jsonl")"
[ "$(printf '%s' "$record" | jq -r '.reason')" = "$REASON" ] ||
  fail "the journal record carries reason $(printf '%s' "$record" | jq -r '.reason'), expected: $REASON"
echo "ok: journal record $record" >>"$REPORT"
[ "$(jq -r ".subscriptions | has(\"$REMEMBERED\")" "$BRAMA_SUBSCRIPTION_USAGE_FILE")" = false ] ||
  fail "the ledger still holds $REMEMBERED: $(cat "$BRAMA_SUBSCRIPTION_USAGE_FILE")"
[ "$(jq -r ".subscriptions | has(\"$LISTED\")" "$BRAMA_SUBSCRIPTION_USAGE_FILE")" = true ] ||
  fail "the ledger lost the listed member too: $(cat "$BRAMA_SUBSCRIPTION_USAGE_FILE")"
echo "ok: the ledger lost exactly the forgotten row" >>"$REPORT"

# --- the pool no longer names it, and forgetting it again is nothing ---
run either subscription list
silent "$REMEMBERED" "the forgotten member left the pool report"
says "$LISTED" "the listed member is still reported"
run refused subscription forget --subscription-id "$REMEMBERED" --reason "$REASON"
says "neither the vault nor the usage history knows \`$REMEMBERED\`" "a forgotten member is unknown afterwards"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
