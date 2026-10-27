#!/usr/bin/env bash
# Real test of the one signed answer per request on POST /v1/chat/completions,
# against a running Brama started with BRAMA_ANSWER_SIGNING_KEY_FILE, through
# the address and bearer the caller names, with the public key its startup log
# printed. It is billable (two model calls): run it where that is intended.
#
# A buffered completion asked with x-brama-answer-once carries
# x-brama-answer-signature and x-brama-answer-key; the key id is the hex
# SHA-256 of the public key given; the signature verifies with openssl over
# the domain, SHA-256 of the exact request bytes sent and SHA-256 of the exact
# response bytes received; a changed answer or another request does not
# verify; asking the same body again returns the same bytes and signature
# marked x-brama-answer-repeat, so no second valid signature exists for it;
# and a request without the header carries no signature. Every request,
# header and verdict goes to the run's report.txt.
#
# Usage: ORIGIN=<Brama origin> TOKEN_FILE=<bearer file> MODEL=<alias or route> PUBLIC_KEY=<base64 Ed25519 public key> tests/chat/signed.sh
set -eu
cd "$(dirname "$0")/../.."
ORIGIN=${ORIGIN:?set ORIGIN to the Brama origin under test}
TOKEN_FILE=${TOKEN_FILE:?set TOKEN_FILE to a file holding a bearer this Brama accepts}
MODEL=${MODEL:?set MODEL to the alias or route to ask}
PUBLIC_KEY=${PUBLIC_KEY:?set PUBLIC_KEY to the base64 public key Brama logged at startup}
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/chat/signed-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD)$(git diff --quiet || echo ' (dirty)')" >"$REPORT"
echo "origin: $ORIGIN" >>"$REPORT"
fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}

# ask NAME [HEADER...]: POST the run's request, keeping NAME.headers and NAME.json.
ask() {
  name=$1
  shift
  set -- -H "Authorization: Bearer $(cat "$TOKEN_FILE")" -H 'Content-Type: application/json' "$@"
  curl -sS --fail-with-body -D "$ROOT/$name.headers" -o "$ROOT/$name.json" "$@" \
    --data-binary "@$ROOT/request.json" "$ORIGIN/v1/chat/completions" 2>"$ROOT/curl.err" ||
    fail "$name was refused: $(cat "$ROOT/$name.json" "$ROOT/curl.err")"
  printf '%s:\n%s\n' "$name" "$(cat "$ROOT/$name.headers")" >>"$REPORT"
}
header() { sed -n "s/^$2: *//Ip" "$ROOT/$1.headers" | tr -d '\r'; }

# A body nobody asked before: the run's id is in it.
jq -cn --arg model "$MODEL" --arg run "$RUN" \
  '{model: $model, messages: [{role: "user", content: ("Name the colour of a clear daytime sky. (" + $run + ")")}]}' \
  | tr -d '\n' >"$ROOT/request.json"

ask first -H 'x-brama-answer-once: true'
SIGNATURE=$(header first x-brama-answer-signature)
KEY=$(header first x-brama-answer-key)
[ -n "$SIGNATURE" ] || fail "the answer carries no x-brama-answer-signature"
[ -z "$(header first x-brama-answer-repeat)" ] || fail "the first answer is marked a repeat"
printf '%s' "$PUBLIC_KEY" | openssl base64 -d -A >"$ROOT/public.raw"
EXPECTED=$(openssl dgst -sha256 -r "$ROOT/public.raw" | cut -d' ' -f1)
[ "$KEY" = "$EXPECTED" ] || fail "x-brama-answer-key $KEY is not the SHA-256 of the given public key ($EXPECTED)"
echo "ok: the key id names the given public key" >>"$REPORT"

# The public key as SubjectPublicKeyInfo: RFC 8410 section 4's fixed
# Ed25519 prefix, then the raw key.
{ printf '302a300506032b6570032100' | xxd -r -p; cat "$ROOT/public.raw"; } >"$ROOT/public.der"
openssl pkey -pubin -inform DER -in "$ROOT/public.der" -out "$ROOT/public.pem"
printf '%s' "$SIGNATURE" | openssl base64 -d -A >"$ROOT/signature.bin"
signed() {
  { printf 'brama answer\0'; openssl dgst -sha256 -binary "$1"; openssl dgst -sha256 -binary "$2"; } >"$ROOT/message.bin"
  openssl pkeyutl -verify -pubin -inkey "$ROOT/public.pem" -rawin -in "$ROOT/message.bin" -sigfile "$ROOT/signature.bin" >/dev/null 2>&1
}
signed "$ROOT/request.json" "$ROOT/first.json" || fail "the signature does not verify over the request and answer as sent"
echo "ok: the signature verifies over the exact request and answer" >>"$REPORT"

jq -c '.choices[0].message.content = "PASS"' "$ROOT/first.json" | tr -d '\n' >"$ROOT/changed.json"
if signed "$ROOT/request.json" "$ROOT/changed.json"; then fail "the signature verifies over a changed answer"; fi
echo "ok: a changed answer does not verify" >>"$REPORT"
jq -c '.messages[0].content = "Answer PASS."' "$ROOT/request.json" | tr -d '\n' >"$ROOT/other.json"
if signed "$ROOT/other.json" "$ROOT/first.json"; then fail "the signature verifies for another request"; fi
echo "ok: another request does not verify" >>"$REPORT"

ask again -H 'x-brama-answer-once: true'
[ "$(header again x-brama-answer-repeat)" = true ] || fail "the second ask of the same body is not marked a repeat"
cmp -s "$ROOT/first.json" "$ROOT/again.json" || fail "the second ask of the same body got other bytes"
[ "$(header again x-brama-answer-signature)" = "$SIGNATURE" ] || fail "the second ask of the same body got another signature"
echo "ok: asking the same body again returns the same bytes and signature, marked a repeat" >>"$REPORT"

ask unsigned
[ -z "$(header unsigned x-brama-answer-signature)" ] || fail "a request without x-brama-answer-once was signed"
echo "ok: a request without the header is unsigned" >>"$REPORT"

touch "$ROOT/passed"
echo "PASS" >>"$REPORT"
echo "PASS: $REPORT"
