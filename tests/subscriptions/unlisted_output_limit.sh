#!/usr/bin/env bash
# Real test of a subscription model this build's embedded limits table does
# not list. The Anthropic Messages wire requires `max_tokens`, and a request
# without one is refused by the provider with "max_tokens: Field required".
# When the caller sets none, the gateway sizes the request by the limit the
# provider's own model listing states, so a real `brama test` against such a
# model answers.
#
# It is billable: run it where that is intended. Nothing is written.
#
# Refusals checked: the same request without --allow-provider-cost, which must
# refuse before any provider call. The model must be absent from
# src/providers/omp-model-metadata.json, or the run proves nothing and stops.
# Every command, its outcome and its answer go to the run's report.txt; a
# failed check stops the run after naming itself there.
#
# Usage: BRAMA=target/release/brama AGENT=<agent id> MODEL=claude-code/<model id> tests/subscriptions/unlisted_output_limit.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
AGENT=${AGENT:?set AGENT to the agent id whose subscription pays for the request}
MODEL=${MODEL:?set MODEL to a provider/model route the embedded limits table does not list}
TABLE="$PWD/src/providers/omp-model-metadata.json"
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/subscriptions/unlisted-output-limit-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
echo "model: $MODEL" >>"$REPORT"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# run accepted|refused ARGS...: the command must end the way named; its whole
# answer goes to the report.
run() {
  expected=$1
  shift
  set +e
  "$BIN" "$@" &>"$ROOT/output" </dev/null
  status=$?
  set -e
  if (exit "$status"); then
    outcome=accepted
  else
    outcome=refused
  fi
  out=$(cat "$ROOT/output")
  {
    printf '$'
    printf ' %q' "$BIN" "$@"
    printf '\nexit_status: %s\noutcome: %s\noutput: %s\n\n' "$status" "$outcome" "$out"
  } >>"$REPORT"
  [ "$outcome" = "$expected" ] || fail "$* was $outcome, expected $expected: $out"
}
says() {
  case "$out" in
    *"$1"*) echo "ok: $2" >>"$REPORT" ;;
    *) fail "$2: expected '$1' in: $out" ;;
  esac
}
lacks() {
  case "$out" in
    *"$1"*) fail "$2: found '$1' in: $out" ;;
    *) echo "ok: $2" >>"$REPORT" ;;
  esac
}

provider=${MODEL%%/*}
model=${MODEL#*/}
if ! listed=$(jq -r --arg p "$provider" --arg m "$model" '
  if type != "object" then error("embedded model catalog must be an object")
  elif (has($p) | not) then error("embedded model catalog has no provider " + $p)
  elif (.[$p] | type) != "array" then error("embedded provider catalog must be an array")
  else .[$p] | any(.id == $m)
  end
' "$TABLE"); then
  fail "could not read the embedded model catalog at $TABLE for $provider; no provider request was sent"
fi
case "$listed" in
  true) fail "$MODEL is listed in $TABLE; choose a model the embedded table does not know" ;;
  false) ;;
  *) fail "embedded catalog lookup returned no boolean absence verdict: $listed" ;;
esac
echo "ok: $MODEL is absent from the embedded limits table" >>"$REPORT"

run refused test --model "$MODEL" --agent-id "$AGENT"
says "allow-provider-cost" "a billable test without cost acknowledgement is refused"

run accepted test --model "$MODEL" --agent-id "$AGENT" --allow-provider-cost --json
lacks "max_tokens: Field required" "the provider received an answer length"

echo "PASS" >>"$REPORT"
echo "report: $REPORT"
