#!/usr/bin/env bash
# Real test of Brama's journal reader through the built CLI (`brama tasks show`
# and `brama tasks list`, which read the same journal retirement, sign-in and
# acquisition decisions are read from). Nothing here calls a provider.
#
# Checked, each in this run's own BRAMA_STATE_DIR so the host's journal is never
# touched:
#   * a journal that does not exist yet is an empty journal: `tasks show`
#     refuses with "no checks are recorded", not with a read failure;
#   * a journal path that cannot be read as a file is refused with the path and
#     the OS cause, never read as empty;
#   * a journal holding a line that is not JSON is refused naming that line;
#   * a readable journal answers with the check it holds.
# Every command, its outcome and its answer go to the run's report.txt; a
# failed check stops the run after naming itself there.
#
# Usage: BRAMA=target/release/brama tests/journal/unreadable.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/journal/unreadable-$RUN"
REPORT="$ROOT/report.txt"
AGENT="journal-test-agent"
TASK="journal-test-task"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"

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
  printf '$ BRAMA_STATE_DIR=%s brama %s\noutcome: %s\noutput: %s\n\n' "$BRAMA_STATE_DIR" "$*" "$outcome" "$out" >>"$REPORT"
  [ "$outcome" = "$expected" ] || fail "$* was $outcome, expected $expected: $out"
}
says() {
  case "$out" in
    *"$1"*) echo "ok: $2" >>"$REPORT" ;;
    *) fail "$2: expected '$1' in: $out" ;;
  esac
}

export BRAMA_STATE_DIR="$ROOT/absent"
mkdir -p "$BRAMA_STATE_DIR"
run refused tasks show "$TASK" --agent-id "$AGENT"
says "no checks are recorded for task $TASK of agent $AGENT" "a journal that does not exist yet is empty"

export BRAMA_STATE_DIR="$ROOT/directory"
mkdir -p "$BRAMA_STATE_DIR/journal.jsonl"
run refused tasks show "$TASK" --agent-id "$AGENT"
says "cannot read the journal $BRAMA_STATE_DIR/journal.jsonl" "an unreadable journal is refused with its path"
run refused tasks list --agent-id "$AGENT"
says "cannot read the journal $BRAMA_STATE_DIR/journal.jsonl" "list refuses the same unreadable journal"

export BRAMA_STATE_DIR="$ROOT/malformed"
mkdir -p "$BRAMA_STATE_DIR"
printf '%s\n' 'this is not a journal record' >"$BRAMA_STATE_DIR/journal.jsonl"
run refused tasks show "$TASK" --agent-id "$AGENT"
says "holds a line that is not JSON (this is not a journal record)" "a malformed line is refused naming the line"

export BRAMA_STATE_DIR="$ROOT/readable"
mkdir -p "$BRAMA_STATE_DIR"
printf '{"kind":"check","agent_id":"%s","provider":"test","model":"test-model","task":"%s","source":"test","status":"active","checked_at":"%s"}\n' \
  "$AGENT" "$TASK" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$BRAMA_STATE_DIR/journal.jsonl"
run accepted tasks show "$TASK" --agent-id "$AGENT" --json
says "\"task\": \"$TASK\"" "a readable journal answers with its check"
says "\"model\": \"test-model\"" "the answered check is the one written"

echo "PASS" >>"$REPORT"
echo "report: $REPORT"
