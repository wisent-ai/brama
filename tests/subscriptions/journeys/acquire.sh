#!/usr/bin/env bash
# Real test of buying a Claude Code account and handing it to omp, through the
# built `brama` against the serving gateway, Weles and the provider. It is
# billable when the pool is spent: then the gateway buys a real account.
#
# First the refusals Brama makes before asking anyone: a provider
# providers.json does not declare, named with the providers it does, and a
# hand-over to a harness the declaration names no mapping for. Then one
# acquisition on the gateway. Its verdict is checked against the state it
# names: `acquired` must leave a new member in the pool that the hand-over
# then signs into omp and `omp usage accounts` lists, and say which shortage
# it bought for (spent, sessions_full or unusable); `refused` must name the
# account or number that stopped it (an account with plan left, or the cap
# with every account counted). Last, the hand-over itself: every pool account
# must end held by omp or named as failed with its reason.
# Every command, its outcome and its answer go to the run's report.txt.
#
# Usage: BRAMA=target/release/brama CONSUMER=<directory consumer> BEARER_ROLE=<console bearer role> \
#   tests/subscriptions/journeys/acquire.sh
set -eu
cd "$(dirname "$0")/../../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
CONSUMER=${CONSUMER:?set CONSUMER to the Stado directory consumer that resolves the gateway}
BEARER_ROLE=${BEARER_ROLE:?set BEARER_ROLE to the role of the console bearer item}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/subscriptions/acquire-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"

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

run undeclared subscription acquire nonexistent-provider --reason "real test: refused provider" --json
[ "$outcome" = refused ] || fail "an acquisition of an undeclared provider was accepted"
grep -q "providers.json declares no subscription provider \`nonexistent-provider\`; it declares claude-code" "$ROOT/undeclared.stderr" ||
  fail "the refusal did not name the missing declaration and the declared providers"

run uncapped subscription acquire codex --reason "real test: a provider declared without a purchase cap" --json
[ "$outcome" = refused ] || fail "an acquisition of a provider without a declared cap was accepted"
[ "$(field uncapped .code)" = account_cap_undeclared ] || fail "the uncapped refusal is not account_cap_undeclared"
field uncapped .detail | grep -q "has not authorized account purchases for codex" ||
  fail "the uncapped refusal did not say no purchase is authorized for codex"

run other-harness subscription hand-over nonexistent-provider --harness omp --json
[ "$outcome" = refused ] || fail "a hand-over of an undeclared provider was accepted"
grep -q "declares no subscription provider \`nonexistent-provider\`" "$ROOT/other-harness.stderr" ||
  fail "the hand-over refusal did not name the missing declaration"

run acquire subscription acquire claude-code --gateway-consumer "$CONSUMER" --bearer-role "$BEARER_ROLE" \
  --reason "real test of automatic account acquisition" --json
result=$(field acquire .result)
code=$(field acquire .code)
echo "acquisition: $result ($code)" >>"$REPORT"
case "$result" in
  acquired)
    [ "$outcome" = accepted ] || fail "an acquired verdict exited unsuccessfully"
    id=$(field acquire .subscription_id)
    account=$(field acquire .account)
    shortage=$(field acquire .shortage)
    request=$(field acquire .request_id)
    [ -n "$request" ] && [ "$request" != null ] || fail "the acquired verdict carries no request_id"
    [ "$id" != "$request" ] || fail "the acquired member is the purchase's correlation id $request, not a member"
    case "$id" in claude-code-?*) ;; *) fail "the acquired member $id is not named <provider>-<account>" ;; esac
    case "$shortage" in spent | sessions_full | unusable) ;; *) fail "the acquisition named no shortage: $shortage" ;; esac
    run list subscription list --gateway-consumer "$CONSUMER" --bearer-role "$BEARER_ROLE" --json
    jq -e --arg id "$id" '[.. | objects | select(.subscription_id? == $id or .id? == $id)] | any' \
      "$ROOT/list.json" >/dev/null || fail "the bought member $id is not in the gateway's pool"
    ;;
  refused)
    [ "$outcome" = refused ] || fail "a refused verdict exited successfully"
    case "$code" in
      pool_not_spent)
        jq -e '[.standings[] | select(.standing == "available")] | any' "$ROOT/acquire.json" >/dev/null ||
          fail "pool_not_spent named no account with plan left"
        ;;
      account_cap_reached)
        jq -e '(.accounts | length) >= .cap' "$ROOT/acquire.json" >/dev/null ||
          fail "account_cap_reached counted fewer accounts than the cap"
        ;;
      *) echo "refusal: $(field acquire .detail)" >>"$REPORT" ;;
    esac
    ;;
  *) fail "the acquisition ended $result ($code): $(field acquire .detail)" ;;
esac

run hand-over subscription hand-over claude-code --harness omp --json
jq -e '.accounts | all(.result == "held" or .result == "handed_over" or ((.detail // "") != ""))' \
  "$ROOT/hand-over.json" >/dev/null || fail "a hand-over row failed without saying why"
if [ "$result" = acquired ]; then
  omp usage accounts >"$ROOT/omp-accounts.txt"
  grep -qi "email:$account|" "$ROOT/omp-accounts.txt" || fail "omp does not hold the bought account $account"
fi

# A session asks for a lease. The gateway either gives one, or, when every
# usable member is full or none can serve, buys for that shortage before it
# answers: a lease answered after a purchase carries the verdict, which must
# be `acquired` and name the shortage; a refusal must carry the verdict of
# the acquisition it started. The session's lease is released afterwards.
SESSION="real-test-acquire-$RUN"
run lease subscription lease take claude-code --session-id "$SESSION" --holder brama-real-test \
  --gateway-consumer "$CONSUMER" --bearer-role "$BEARER_ROLE" --json
if [ "$outcome" = accepted ]; then
  [ "$(field lease .lease.session_id)" = "$SESSION" ] || fail "the lease answered is not this session's"
  if [ "$(field lease '.acquisition != null')" = true ]; then
    [ "$(field lease .acquisition.result)" = acquired ] ||
      fail "a lease answered after a purchase carries a verdict that is not acquired"
    case "$(field lease .acquisition.shortage)" in sessions_full | spent | unusable) ;;
      *) fail "the purchase behind the lease named no shortage" ;;
    esac
  fi
  run release subscription lease release --session-id "$SESSION" \
    --gateway-consumer "$CONSUMER" --bearer-role "$BEARER_ROLE" --json
  [ "$outcome" = accepted ] || fail "the test session's lease could not be released"
else
  grep -Eq 'pool_full|no_usable_member|limit_unstated|dependency_unavailable|invalid_request' "$ROOT/lease.stderr" ||
    fail "the lease refusal named no code"
  if grep -Eq 'pool_full|no_usable_member' "$ROOT/lease.stderr"; then
    grep -q '"acquisition"' "$ROOT/lease.stderr" ||
      fail "a lease refused for a shortage carries no acquisition verdict"
  fi
fi

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
