#!/usr/bin/env bash
# List one agent's subscriptions through the signed route:
#   GET /v1/subscriptions/<agent>  with bearer + the x-agent-* HMAC trio.
#
# Signature scheme (src/crypto/hmac_auth.rs): HMAC-SHA256 hex over
#   "{agent_id}:{timestamp}:{body_sha256_hex}"
# where the timestamp is unix seconds (accepted within a ±300s window) and
# the body hash is the empty string for a bodyless GET.
#
# The gateway must know the agent's signing secret:
#   BRAMA_REQUEST_SIGN_IDENTITIES='{"wisent-app":"<secret>"}'
# (standalone-serve-with-stub.sh sets it to docs-signing-secret).
#
# Usage:
#   ./signed-agent-listing.sh
#   BRAMA_URL=http://127.0.0.1:18321 BRAMA_TOKEN=docs-test-token \
#     BRAMA_AGENT=wisent-app BRAMA_AGENT_SECRET=docs-signing-secret \
#     ./signed-agent-listing.sh
set -euo pipefail

BRAMA_URL="${BRAMA_URL:-http://127.0.0.1:18321}"
BRAMA_TOKEN="${BRAMA_TOKEN:-docs-test-token}"
BRAMA_AGENT="${BRAMA_AGENT:-wisent-app}"
BRAMA_AGENT_SECRET="${BRAMA_AGENT_SECRET:-docs-signing-secret}"

ts="$(date +%s)"
body_hash=""    # bodyless GET signs the empty string
sig="$(printf %s "${BRAMA_AGENT}:${ts}:${body_hash}" \
  | openssl dgst -sha256 -hmac "${BRAMA_AGENT_SECRET}" -hex \
  | awk '{print $NF}')"

curl -sS \
  -H "Authorization: Bearer ${BRAMA_TOKEN}" \
  -H "x-agent-id: ${BRAMA_AGENT}" \
  -H "x-agent-timestamp: ${ts}" \
  -H "x-agent-signature: ${sig}" \
  "${BRAMA_URL}/v1/subscriptions/${BRAMA_AGENT}"
echo
