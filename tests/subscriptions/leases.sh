#!/usr/bin/env bash
# Real test of subscription leases through the built `brama` against the
# serving gateway: a lease is taken for a session on the usable member
# carrying the fewest sessions, the same session asking again gets its
# standing lease back, the pool report and the lease list carry the counts,
# and a release ends it. Then the refusals the gateway makes before any lease
# is written: a lease that names no session, a release that names nothing, a
# provider the pool holds no usable member of. Last, a full pool: sessions
# are leased on FULL_PROVIDER until the gateway refuses the next one
# `pool_full`, with every usable subscription carrying the operator's limit,
# and releasing them frees the pool again. FULL_PROVIDER is a provider the
# declaration names no purchase cap for, so the acquisition the full pool
# starts is refused `account_cap_undeclared` and nothing is bought. Every
# command, its outcome and its answer go to the run's report.txt.
#
# Usage: BRAMA=target/release/brama CONSUMER=<directory consumer> BEARER_ROLE=<console bearer role> \
#   PROVIDER=<provider with a usable member> FULL_PROVIDER=<declared provider without a purchase cap> \
#   tests/subscriptions/leases.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
CONSUMER=${CONSUMER:?set CONSUMER to the Stado directory consumer that resolves the gateway}
BEARER_ROLE=${BEARER_ROLE:?set BEARER_ROLE to the role of the console bearer item}
PROVIDER=${PROVIDER:?set PROVIDER to a provider whose pool holds a usable member}
FULL_PROVIDER=${FULL_PROVIDER:?set FULL_PROVIDER to a declared provider with usable members and no purchase cap, e.g. codex}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/subscriptions/leases-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
SESSION="real-test-leases-$RUN"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# run NAME ARGS...: run brama, keep its stdout in $ROOT/NAME.json and its
# stderr beside it, and record the outcome; never stops on its own.
run() {
  name=$1
  shift
  if "$BIN" "$@" >"$ROOT/$name.json" 2>"$ROOT/$name.stderr" </dev/null; then outcome=accepted; else outcome=refused; fi # https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html
  printf '$ brama %s\noutcome: %s\nstdout: %s\nstderr: %s\n\n' "$*" "$outcome" \
    "$(cat "$ROOT/$name.json")" "$(cat "$ROOT/$name.stderr")" >>"$REPORT"
}
field() {
  jq -r "$2" "$ROOT/$1.json"
}
says() {
  grep -qF -- "$2" "$ROOT/$1.stderr" || fail "$3: expected '$2' in: $(cat "$ROOT/$1.stderr")"
  echo "ok: $3" >>"$REPORT"
}
at_gateway=(--gateway-consumer "$CONSUMER" --bearer-role "$BEARER_ROLE")

# --- refusals before any lease is written ---
run no-session subscription lease take "$PROVIDER" --session-id "" --holder real-test "${at_gateway[@]}" --json
[ "$outcome" = refused ] || fail "a lease with an empty session id was accepted"
run no-name subscription lease release "${at_gateway[@]}" --json
[ "$outcome" = refused ] || fail "a release naming nothing was accepted"
says no-name "required" "a release names the lease or the session"
run no-member subscription lease take "no-such-provider" --session-id "$SESSION" --holder real-test "${at_gateway[@]}" --json
[ "$outcome" = refused ] || fail "a lease on a provider with no member was accepted"
says no-member "no_usable_member" "a provider with no usable member is refused by name"

# --- one lease, taken once ---
run take subscription lease take "$PROVIDER" --session-id "$SESSION" --holder real-test "${at_gateway[@]}" --json
[ "$outcome" = accepted ] || fail "the lease was refused: $(cat "$ROOT/take.stderr")"
lease=$(field take .lease.id)
subscription=$(field take .lease.subscription_id)
[ "$(field take .new)" = true ] || fail "the first lease of the session is not new"
[ -n "$lease" ] && [ -n "$subscription" ] || fail "the lease names no id or subscription"
echo "lease $lease on $subscription" >>"$REPORT"

run again subscription lease take "$PROVIDER" --session-id "$SESSION" --holder real-test "${at_gateway[@]}" --json
[ "$outcome" = accepted ] || fail "the second ask was refused: $(cat "$ROOT/again.stderr")"
[ "$(field again .new)" = false ] || fail "the session's second ask made a second lease"
[ "$(field again .lease.id)" = "$lease" ] || fail "the session's second ask answered another lease"

# --- the registers agree ---
run list subscription lease list "${at_gateway[@]}" --json
jq -e --arg id "$lease" '.leases[] | select(.id == $id)' "$ROOT/list.json" >/dev/null ||
  fail "the lease list does not carry $lease"
listed=$(jq -r --arg s "$subscription" '.counts[$s]' "$ROOT/list.json")
run pool subscription list "${at_gateway[@]}" --json
reported=$(jq -r --arg s "$subscription" '.subscriptions[] | select(.id == $s) | .live_sessions' "$ROOT/pool.json")
[ "$listed" = "$reported" ] || fail "the lease list says $subscription carries $listed, the pool report says $reported"
[ "$(field take .live_on_subscription)" = "$listed" ] ||
  fail "the lease said $subscription carries $(field take .live_on_subscription), the list says $listed"
echo "ok: $subscription carries $listed in the lease list, the pool report and the lease" >>"$REPORT"

# --- release, and the registers agree again ---
run release subscription lease release --session-id "$SESSION" "${at_gateway[@]}" --json
[ "$outcome" = accepted ] || fail "the release was refused: $(cat "$ROOT/release.stderr")"
jq -e --arg id "$lease" '.released[] | select(.id == $id)' "$ROOT/release.json" >/dev/null ||
  fail "the release did not name $lease"
run released-again subscription lease release --session-id "$SESSION" "${at_gateway[@]}" --json
[ "$outcome" = accepted ] || fail "a repeated release was refused"
[ "$(jq -r '.released | length' "$ROOT/released-again.json")" = "$(jq -n '[] | length')" ] ||
  fail "a repeated release released something again"
run list-after subscription lease list "${at_gateway[@]}" --json
jq -e --arg id "$lease" '.leases[] | select(.id == $id)' "$ROOT/list-after.json" >/dev/null &&
  fail "the released lease $lease is still listed"

# --- a full pool refuses the next session, and releases free it ---
FULL="$ROOT/full-sessions.txt"
: >"$FULL"
release_full() {
  while read -r held; do
    run "release-$held" subscription lease release --session-id "$held" "${at_gateway[@]}" --json
  done <"$FULL"
  : >"$FULL"
}
trap release_full EXIT
while :; do
  next="real-test-full-$RUN-$(uuidgen | tr 'A-Z' 'a-z')"
  run "$next" subscription lease take "$FULL_PROVIDER" --session-id "$next" --holder real-test "${at_gateway[@]}" --json
  [ "$outcome" = accepted ] || break
  echo "$next" >>"$FULL"
done
[ -s "$FULL" ] || fail "no lease could be taken on $FULL_PROVIDER: $(cat "$ROOT/$next.stderr")"
says "$next" "pool_full" "the session after the pool filled is refused pool_full"
says "$next" "account_cap_undeclared" "the acquisition the full pool started bought nothing for an uncapped provider"
run list-full subscription lease list "${at_gateway[@]}" --json
limit=$(jq -r --arg p "$FULL_PROVIDER" '.limits[$p]' "$ROOT/list-full.json")
jq -e --arg p "$FULL_PROVIDER" --argjson limit "$limit" \
  '[.leases[] | select(.provider == $p) | .subscription_id] | group_by(.) | all(length >= $limit)' \
  "$ROOT/list-full.json" >/dev/null || fail "a $FULL_PROVIDER subscription carries fewer than $limit sessions while the pool refused one"
echo "ok: $(wc -l <"$FULL") sessions filled every usable $FULL_PROVIDER subscription to $limit" >>"$REPORT"
release_full
trap - EXIT
run after-full subscription lease take "$FULL_PROVIDER" --session-id "$next" --holder real-test "${at_gateway[@]}" --json
[ "$outcome" = accepted ] || fail "the pool still refused a session after the full sessions were released: $(cat "$ROOT/after-full.stderr")"
run release-after subscription lease release --session-id "$next" "${at_gateway[@]}" --json

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
