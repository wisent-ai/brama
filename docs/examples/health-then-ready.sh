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
# -w prints the status: 200 means at least one route can serve (inspect
# `degraded` for partial failures); 503 means no configured route can serve.
curl -sS -w '\nHTTP %{http_code}\n' "${BRAMA_URL}/readyz"
