#!/usr/bin/env bash
# List one agent's subscriptions through the signed route:
#   GET /v1/subscription-pool  with bearer + the x-agent-* HMAC trio.
#
# Signature scheme (src/crypto/hmac_auth.rs): HMAC-SHA256 hex over
#   "{agent_id}:{timestamp}:{body_sha256_hex}"
# where the timestamp is unix seconds (accepted within a ±300s window) and
# the body hash is the empty string for a bodyless GET.
#
# The gateway must know the agent's signing secret through
# BRAMA_REQUEST_SIGN_IDENTITIES='{"<agent>":"<secret>"}'.
#
# Usage:
#   BRAMA_URL=<gateway origin> BRAMA_TOKEN=<client bearer> BRAMA_AGENT=<agent> \
#     BRAMA_AGENT_SECRET=<secret> ./signed-agent-listing.sh
set -euo pipefail

: "${BRAMA_URL:?BRAMA_URL must name the gateway to ask}"
: "${BRAMA_TOKEN:?BRAMA_TOKEN must hold the client bearer the gateway issued}"
: "${BRAMA_AGENT:?BRAMA_AGENT must name the agent whose subscriptions are listed}"
: "${BRAMA_AGENT_SECRET:?BRAMA_AGENT_SECRET must hold that agent's request-sign secret}"

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
  "${BRAMA_URL}/v1/subscription-pool"
echo
