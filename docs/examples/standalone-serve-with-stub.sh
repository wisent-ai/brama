#!/usr/bin/env bash
# Start a standalone Brama gateway on loopback against the local stub
# provider — no Skarbiec deployment, no real credential, no provider spend.
#
# The stub (stub-provider.py) listens on 127.0.0.1:18999 and answers the
# OpenAI wire shape for models stub-ok, stub-401, and stub-429. The gateway
# is pointed at it through the per-provider base-URL override, and all
# durable state lands in a fresh temp directory, so nothing on the machine
# is read or changed.
#
# Usage:
#   ./standalone-serve-with-stub.sh              # brama from PATH, port 18321
#   BRAMA_BIN=target/debug/brama BRAMA_PORT=18321 ./standalone-serve-with-stub.sh
#
# The gateway runs in the foreground; stop it with Ctrl-C (the stub is
# cleaned up on exit). Then, from another shell:
#   ./health-then-ready.sh
#   ./signed-agent-listing.sh
set -euo pipefail

BRAMA_BIN="${BRAMA_BIN:-brama}"
BRAMA_PORT="${BRAMA_PORT:-18321}"
STATE="$(mktemp -d)"
HERE="$(cd "$(dirname "$0")" && pwd)"
echo "isolated state: ${STATE}" >&2

python3 "${HERE}/stub-provider.py" &
STUB_PID=$!
trap 'kill "${STUB_PID}" 2>/dev/null || true' EXIT

# Point the openai adapter at the loopback stub (explicit deployment
# override; loopback is otherwise refused as a provider endpoint).
export BRAMA_PROVIDER_OPENAI_BASE_URL="http://127.0.0.1:18999"

# One static client identity: requests authenticate with
# `Authorization: Bearer docs-test-token`.
export BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES='[{"client_id":"docs-test","token":"docs-test-token"}]'

# A request-sign secret for the wisent-app agent so signed-agent-listing.sh
# can compute the HMAC trio. Docs value — not a credential.
export BRAMA_REQUEST_SIGN_IDENTITIES='{"wisent-app":"'"${BRAMA_AGENT_SECRET:-docs-signing-secret}"'"}'

# Full state isolation: journal, usage ledger, donated-subscription file,
# perf persistence, and the entitlements-router lookup all point into the
# temp directory, so no operator state is touched.
export BRAMA_STATE_DIR="${STATE}"
export BRAMA_SUBSCRIPTION_USAGE_FILE="${STATE}/subscription-usage.json"
export BRAMA_DONATED_SUBSCRIPTIONS_FILE="${STATE}/donated-subscriptions.json"
export BRAMA_PERF_PATH="${STATE}/perf.json"
export ENTITLEMENTS_ROUTER_BIN="${STATE}/entitlements-router-absent"

# The credential is syntactically a bearer and semantically garbage: the
# stub never checks it, which is the point — nothing here can spend money.
printf %s '{"openai": "sk-brama-docs-invalid"}' | \
  "${BRAMA_BIN}" serve --port "${BRAMA_PORT}" --local-credentials-stdin
