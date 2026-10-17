#!/usr/bin/env bash
# Real test of `response_format` on POST /v1/chat/completions, against a
# running Brama through the address and bearer the caller names. It is
# billable: run it where that is intended.
#
# Accepted: a `json_schema` request whose answer must be one JSON object
# holding the schema's required key. Refused before any provider call: a
# `text` format, a `json_schema` without its object, `strict: false`, and a
# streamed request. Every request, its status and its answer go to the run's
# report.txt; a failed check stops the run after naming itself there.
#
# Usage: ORIGIN=<Brama origin> TOKEN_FILE=<file holding the bearer> MODEL=<alias or route> tests/chat/response_format.sh
set -eu
cd "$(dirname "$0")/../.."
ORIGIN=${ORIGIN:?set ORIGIN to the Brama origin under test}
TOKEN_FILE=${TOKEN_FILE:?set TOKEN_FILE to a file holding a bearer this Brama accepts}
MODEL=${MODEL:?set MODEL to the alias or route to ask}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/chat/response-format-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "origin: $ORIGIN" >>"$REPORT"
echo "model: $MODEL" >>"$REPORT"
echo "served: $(curl -sS "$ORIGIN/health")" >>"$REPORT"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
# ask accepted|refused BODY: POST BODY; `curl --fail-with-body` tells an
# answered request from a refused one, and the answer is left in
# $ROOT/answer.json and $out.
ask() {
  expected=$1
  if curl -sS --fail-with-body -o "$ROOT/answer.json" \
    -H "Authorization: Bearer $(cat "$TOKEN_FILE")" -H 'Content-Type: application/json' \
    --data "$2" "$ORIGIN/v1/chat/completions" 2>"$ROOT/curl.err"; then outcome=accepted; else outcome=refused; fi
  out="$(cat "$ROOT/answer.json") $(cat "$ROOT/curl.err")"
  printf 'POST /v1/chat/completions %s\noutcome: %s\nanswer: %s\n\n' "$2" "$outcome" "$out" >>"$REPORT"
  [ "$outcome" = "$expected" ] || fail "expected $expected, got $outcome: $out"
}
says() {
  case "$out" in
    *"$1"*) echo "ok: $2" >>"$REPORT" ;;
    *) fail "$2: expected '$1' in: $out" ;;
  esac
}

SCHEMA='{"type":"object","additionalProperties":false,"required":["colour"],"properties":{"colour":{"type":"string"}}}'
PROMPT='Name the colour of a clear daytime sky.'
request() {
  jq -cn --arg model "$MODEL" --arg prompt "$PROMPT" --argjson format "$1" --argjson stream "$2" \
    '{model: $model, messages: [{role: "user", content: $prompt}], response_format: $format, stream: $stream}'
}
STRICT="{\"type\":\"json_schema\",\"json_schema\":{\"name\":\"sky\",\"strict\":true,\"schema\":$SCHEMA}}"

ask accepted "$(request "$STRICT" false)"
jq -r '.choices[0].message.content' "$ROOT/answer.json" >"$ROOT/content.json"
# The answer's keys are exactly the schema's required ones.
jq -e --argjson schema "$SCHEMA" 'keys == ($schema.required | sort)' "$ROOT/content.json" >/dev/null ||
  fail "the answer is not the object the schema names: $(cat "$ROOT/content.json")"
echo "ok: the answer is one JSON object holding exactly the schema's keys" >>"$REPORT"

ask refused "$(request '{"type":"text"}' false)"
says "only \`json_schema\`" "a text format is refused"

ask refused "$(request '{"type":"json_schema"}' false)"
says "carries no \`json_schema\` object" "a json_schema format without its object is refused"

ask refused "$(request "{\"type\":\"json_schema\",\"json_schema\":{\"name\":\"sky\",\"strict\":false,\"schema\":$SCHEMA}}" false)"
says "strict: false" "a non-strict schema is refused"

ask refused "$(request "$STRICT" true)"
says "answered buffered only" "a streamed request with a schema is refused"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
