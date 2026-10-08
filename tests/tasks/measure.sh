#!/usr/bin/env bash
# Real test of `brama tasks`: measure one named task through the agent's real
# subscription routes with --persist, then read the recorded checks back with
# `tasks show` and `tasks list`, the evidence `task:<KEY>` selection serves
# from. It is billable: run it where that is intended. The journal lives in
# this run's own BRAMA_STATE_DIR, so the host's journal is never touched.
#
# Refusals checked: `tasks show` of a task nobody measured, and `tasks measure`
# without --allow-provider-cost, which must refuse before any provider call.
# Every command, its outcome and its answer go to the run's report.txt; a
# failed check stops the run after naming itself there.
#
# Usage: BRAMA=target/release/brama AGENT=<agent id> MAX_MODELS=<count> tests/tasks/measure.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
AGENT=${AGENT:?set AGENT to the Jeden agent id whose subscription routes are measured}
MAX_MODELS=${MAX_MODELS:?set MAX_MODELS to how many active models may be checked; each is one billable request}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/tasks/measure-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT/state"
export BRAMA_STATE_DIR="$ROOT/state"
TASK="real-test-$RUN"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
echo "agent: $AGENT" >>"$REPORT"
echo "task: $TASK" >>"$REPORT"

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

run refused tasks show "$TASK" --agent-id "$AGENT"
says "no checks are recorded for task $TASK of agent $AGENT" "an unmeasured task is refused"

run refused tasks measure "$TASK" --agent-id "$AGENT" --prompt 'Reply with the single word ready.' \
  --expected-contains ready --max-models "$MAX_MODELS" --persist
says "without explicit --allow-provider-cost" "measure without cost acknowledgement is refused"
[ ! -s "$BRAMA_STATE_DIR/journal.jsonl" ] || fail "the refused measure wrote the journal"
echo "ok: the refused measure wrote nothing" >>"$REPORT"

run accepted tasks measure "$TASK" --agent-id "$AGENT" --prompt 'Reply with the single word ready.' \
  --expected-contains ready --max-models "$MAX_MODELS" --persist --allow-provider-cost --json

run accepted tasks show "$TASK" --agent-id "$AGENT" --json
says "\"task\": \"$TASK\"" "show names the measured task"
says "\"checked_at\"" "show prints the recorded checks"

run accepted tasks list --agent-id "$AGENT" --json
says "\"task\": \"$TASK\"" "list names the measured task"
says "\"newest\"" "list reports when the newest check was taken"

echo "PASS" >>"$REPORT"
echo "report: $REPORT"
