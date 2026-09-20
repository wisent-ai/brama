#!/usr/bin/env bash
# Liveness, then readiness — the two unauthenticated routes, in the order an
# operator should read them. /health proves only that the process is up;
# /readyz redeems real credentials and reports both whether any route can carry
# traffic and whether the remaining configured accounts are degraded.
#
# Usage:
#   ./health-then-ready.sh
#   BRAMA_URL=http://127.0.0.1:18321 ./health-then-ready.sh
set -euo pipefail

BRAMA_URL="${BRAMA_URL:-http://127.0.0.1:18321}"

echo "== GET ${BRAMA_URL}/health"
curl -sS "${BRAMA_URL}/health"
echo; echo

echo "== GET ${BRAMA_URL}/readyz"
# -w prints the status. 200 means this deployment is installable: either a
# route can serve — inspect `ready` and `degraded` for partial failures — or
# it is configured and every credential is dead, in which case `ready` is
# false, `operator_action_required` is true and the reason says it is waiting
# for a sign-in. 503 means nothing is configured at all, so there is nothing
# to sign in and nothing to serve.
curl -sS -w '\nHTTP %{http_code}\n' "${BRAMA_URL}/readyz"
