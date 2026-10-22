#!/usr/bin/env bash
# Real test of Brama's side of the contribution ledger, against a real ledger
# (compute.wisent.com's API with its database) and a running gateway that
# declares it: Brama links no identity (the route that did answers nothing,
# for a signed-in user, a workload caller or the console alike);
# `brama rewards status` and the console's `POST /v1/rewards/status` answer
# the same report about a signed-in user's entitlement to a gated alias, the
# user known by their Wisent account id; an ungated alias is not gated; a
# gateway with no ledger says so by name. Nothing is written.
#
# Usage:
#   BRAMA=target/release/brama LEDGER=<ledger origin> \
#   USER_ID=<a Wisent user id whose account linked itself for brama> GATED=<an alias to gate> \
#   ENTITLED=<true|false> GATEWAY=<gateway origin> \
#   USER_BEARER=<USER's signed-in Wisent token> \
#   CONSOLE_BEARER=<its console's bearer> CALLER_BEARER=<a workload caller's bearer> \
#   tests/rewards/ledger.sh
#
# The Skarbiec route for the `contribution-ledger` resource must name the
# bearer the ledger's operator declared for the reporter `brama`. ENTITLED
# says whether USER's account holds model.access for GATED, so the
# entitlement the ledger answers is checked, not just printed.
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test}
LEDGER_ORIGIN=${LEDGER:?set LEDGER to the contribution ledger origin under test}
USER_UUID=${USER_ID:?set USER_ID to the Wisent user id USER_BEARER signs in as}
GATED_ALIAS=${GATED:?set GATED to the alias the gateway gates in this run}
EXPECT=${ENTITLED:?set ENTITLED to true or false: whether the account holds model.access for GATED}
GATEWAY_ORIGIN=${GATEWAY:?set GATEWAY to the origin of a running gateway that declares LEDGER and gates GATED}
: "${USER_BEARER:?set USER_BEARER to the signed-in Wisent token of USER_ID}"
: "${CONSOLE_BEARER:?set CONSOLE_BEARER to the bearer of the console of GATEWAY}"
: "${CALLER_BEARER:?set CALLER_BEARER to the bearer of a workload caller of GATEWAY}"
# The ledger knows a signed-in user by their Wisent account id.
AGENT_ID="$USER_UUID"
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/rewards/ledger-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
echo "ledger: $LEDGER_ORIGIN" >>"$REPORT"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# run accepted|refused ARGS...: the command must end the way named.
run() {
  expected=$1
  shift
  if "$BIN" "$@" &>"$ROOT/output" </dev/null; then outcome=accepted; else outcome=refused; fi
  out=$(cat "$ROOT/output")
  printf '$ brama %s\noutcome: %s\noutput: %s\n\n' "$*" "$outcome" "$out" >>"$REPORT"
  [ "$outcome" = "$expected" ] || fail "$* was $outcome, expected $expected: $out"
}
says() {
  case "$out" in
    *"$1"*) echo "ok: $2" >>"$REPORT" ;;
    *) fail "$2: expected '$1' in: $out" ;;
  esac
}
# post accepted|refused BEARER ROUTE BODY: the gateway must answer the way
# named; the bearer travels in curl's configuration on standard input.
post() {
  expected=$1
  if printf 'header = "Authorization: Bearer %s"\n' "$2" |
    curl --silent --show-error --fail-with-body --config - -H 'Content-Type: application/json' \
      --data "$4" "$GATEWAY_ORIGIN$3" &>"$ROOT/output"; then outcome=accepted; else outcome=refused; fi
  out=$(cat "$ROOT/output")
  printf 'POST %s %s\noutcome: %s\noutput: %s\n\n' "$3" "$4" "$outcome" "$out" >>"$REPORT"
  [ "$outcome" = "$expected" ] || fail "POST $3 was $outcome, expected $expected: $out"
}

# Without a ledger the operator's shell reports none, by its setting.
unset BRAMA_CONTRIBUTION_LEDGER_URL
run accepted rewards status --json
says 'none declared (BRAMA_CONTRIBUTION_LEDGER_URL)' "status names the missing ledger"

# Brama links no identity: the route that did is gone for every caller.
linking=$(jq -cn '{code: "unspent"}')
for bearer in "$USER_BEARER" "$CALLER_BEARER" "$CONSOLE_BEARER"; do
  post refused "$bearer" /v1/rewards/link "$linking"
done
echo "ok: no caller links an identity through Brama" >>"$REPORT"

export BRAMA_CONTRIBUTION_LEDGER_URL="$LEDGER_ORIGIN"
export BRAMA_ENTITLEMENT_GATED_ALIASES="$GATED_ALIAS"
run accepted rewards status --alias "$GATED_ALIAS" --agent-id "$AGENT_ID" --json
jq -e '.gated == true' "$ROOT/output" >/dev/null || fail "$GATED_ALIAS is not reported gated: $out"
jq -e --argjson expect "$EXPECT" '.entitlement.granted == $expect' "$ROOT/output" >/dev/null ||
  fail "the ledger's entitlement answer is not granted=$EXPECT: $out"
if [ "$EXPECT" = false ]; then
  jq -e '.entitlement.reason == "entitlement_absent"' "$ROOT/output" >/dev/null ||
    fail "a denial does not name entitlement_absent: $out"
fi
echo "ok: entitlement to $GATED_ALIAS answered granted=$EXPECT" >>"$REPORT"

run accepted rewards status --alias "$GATED_ALIAS-ungated" --json
jq -e '.gated == false and (has("entitlement") | not)' "$ROOT/output" >/dev/null ||
  fail "an ungated alias is reported gated or asked about: $out"
echo "ok: an ungated alias is not gated" >>"$REPORT"

report_body=$(jq -cn --arg alias "$GATED_ALIAS" --arg agent "$AGENT_ID" '{alias: $alias, agent_id: $agent}')
post accepted "$CONSOLE_BEARER" /v1/rewards/status "$report_body"
jq -e --arg ledger "$LEDGER_ORIGIN" --argjson expect "$EXPECT" \
  '.ledger == $ledger and .gated == true and .entitlement.granted == $expect' "$ROOT/output" >/dev/null ||
  fail "the console's report is not the ledger's answer granted=$EXPECT: $out"
echo "ok: the console reads the same report" >>"$REPORT"
post refused "$CALLER_BEARER" /v1/rewards/status "$report_body"
says "not this gateway's console" "a caller that is not the console is refused the report"

echo "PASS" >>"$REPORT"
echo "Rewards ledger evidence: $REPORT"
