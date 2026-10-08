#!/usr/bin/env bash
# Real test of image generation on OpenRouter's image API through the built
# `brama image`, against OpenRouter itself with the credential this host's
# Brama resolves for it. It is billable: run it where that is intended.
#
# It renders one picture from a prompt and one from the checked-in banner as
# a reference image, and checks each written file is a picture. Then the
# refusal OpenRouter's contract makes Brama state before any provider call: a
# mask, which OpenRouter's image API does not document. Every command, its
# outcome and its answer go to the run's report.txt; a failed check stops the
# run after naming itself there.
#
# Usage: BRAMA=target/release/brama ROUTE=openrouter/<vendor>/<image model> tests/media/openrouter.sh
# (`brama models --kind image --provider openrouter` lists the routes).
set -eu
cd "$(dirname "$0")/../.."
BIN=${BRAMA:?set BRAMA to the brama binary under test, e.g. BRAMA=target/release/brama}
ROUTE=${ROUTE:?set ROUTE to an OpenRouter image route; brama models --kind image --provider openrouter lists them}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/media/openrouter-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "binary: $BIN" >>"$REPORT"
echo "route: $ROUTE" >>"$REPORT"

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
picture() {
  case "$(file -b --mime-type "$2")" in
    image/*) echo "ok: $1 is $(file -b --mime-type "$2")" >>"$REPORT" ;;
    *) fail "$1 is not a picture: $(file -b "$2")" ;;
  esac
}
refused() {
  case "$out" in
    *"$1"*) echo "ok: refused with: $1" >>"$REPORT" ;;
    *) fail "expected a refusal containing '$1', got: $out" ;;
  esac
}

BANNER=assets/readme-banner.webp
run accepted image --model "$ROUTE" --prompt "a small red bicycle leaning on a white wall, flat illustration" \
  --output "$ROOT/generated" --allow-provider-cost
picture "the generated picture" "$ROOT/generated"
run accepted image --model "$ROUTE" --prompt "the same picture as a pencil sketch" --image "$BANNER" \
  --output "$ROOT/referenced" --allow-provider-cost
picture "the picture made from a reference" "$ROOT/referenced"

run refused image --model "$ROUTE" --prompt "repaint the masked part" --image "$BANNER" --mask "$BANNER" \
  --output "$ROOT/masked" --allow-provider-cost
refused "takes no \`mask\`"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
