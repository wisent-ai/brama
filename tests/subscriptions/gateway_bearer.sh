#!/usr/bin/env bash
# Real test of how `brama subscription list` settles a remote gateway call,
# through the built binary. A call that names a gateway but carries no
# console bearer is refused for the bearer before anything is asked of
# Stado: naming a consumer Stado's service directory has never heard of
# still answers the bearer refusal, not a directory lookup. A bearer role
# without a gateway is refused naming the gateway it needs.
# Every command, its outcome and its answer go to the run's report.txt; a
# failed check stops the run after naming itself there.
#
# Usage: BRAMA=target/release/brama tests/subscriptions/gateway_bearer.sh
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/subscriptions/gateway-bearer-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
UNKNOWN_CONSUMER="gateway-bearer-journey-$RUN"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# refused ARGS...: the command must be refused, reading an empty stdin; its
# whole answer goes to the report.
refused() {
  if "$BIN" "$@" &>"$ROOT/output" </dev/null; then outcome=accepted; else outcome=refused; fi
  out=$(cat "$ROOT/output")
  printf '$ brama %s\noutcome: %s\noutput: %s\n\n' "$*" "$outcome" "$out" >>"$REPORT"
  [ "$outcome" = refused ] || fail "$* was accepted: $out"
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

refused subscription list --gateway-consumer "$UNKNOWN_CONSUMER" --json
says "a gateway needs the console's bearer" "a consumer's gateway without a bearer is refused for the bearer"
silent "$UNKNOWN_CONSUMER" "Stado was not asked for a consumer the call carried no bearer for"

refused subscription list --gateway "https://gateway.invalid" --json
says "a gateway needs the console's bearer" "an origin without a bearer is refused for the bearer"

refused subscription list --bearer-role console --json
says "--gateway" "a bearer role without a gateway is refused naming the gateway it needs"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
