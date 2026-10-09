#!/usr/bin/env bash
# Real test of `brama decide` on the chat-distribution engine: the two typed
# questions a message reader asks together — what the message is (`kind`) and
# whether it is frustrated (`frustration`), whose labels a small model used to
# write under the wrong question — asked of the decision alias's real route
# over states that invite the mix-up. Every answer must come back inside the
# declared schema: each question answered, its choice one of its own labels.
# It is billable: run it where that is intended. Nothing is written.
#
# Refusals checked: a decision without --allow-provider-cost, which must
# refuse before any provider call, and questions that are not JSON. Every
# command, its outcome and its answer go to the run's report.txt; a failed
# check stops the run after naming itself there.
#
# Usage: BRAMA=target/release/brama AGENT=<agent id> MODEL=<decision-model|best-decision-model> tests/decisions/decide.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
AGENT=${AGENT:?set AGENT to the agent id whose subscription pays when the alias resolves to best}
MODEL=${MODEL:?set MODEL to the decision alias under test: decision-model or best-decision-model}
QUESTIONS="$PWD/tests/decisions/questions.json"
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/decisions/decide-$RUN"
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
# inside QUESTION: the answer document's choice for QUESTION is one of the
# labels questions.json declares for it.
inside() {
  sed -n '/^{/,$p' "$ROOT/output" >"$ROOT/answer.json"
  choice=$(jq -r --arg q "$1" '.answers[$q].choice // empty' "$ROOT/answer.json")
  [ -n "$choice" ] || fail "question $1 has no choice: $out"
  jq -e --arg q "$1" --arg c "$choice" '.[$q].criteria | has($c)' "$QUESTIONS" >/dev/null ||
    fail "question $1 answered $choice, which it does not declare"
  echo "ok: $1 answered $choice, one of its labels" >>"$REPORT"
}

run refused decide --model "$MODEL" --agent-id "$AGENT" --state 'Fix the login page.' --questions @"$QUESTIONS"
says "refusing a billable decision without explicit --allow-provider-cost" "a decision without cost acknowledgement is refused"

run refused decide --model "$MODEL" --agent-id "$AGENT" --state 'Fix the login page.' --questions 'not json' \
  --allow-provider-cost
says "--questions is not JSON" "questions that do not parse are refused"

for state in \
  'Fix the login page so a wrong password says which field is wrong.' \
  'Why is this still not done? I asked three times. Do it now.' \
  'Thanks, that works.' \
  'Is the build green?'; do
  run accepted decide --model "$MODEL" --agent-id "$AGENT" --state "$state" --questions @"$QUESTIONS" \
    --allow-provider-cost --json
  inside kind
  inside frustration
done

echo "PASS" >>"$REPORT"
echo "report: $REPORT"
