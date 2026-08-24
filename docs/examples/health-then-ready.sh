#!/usr/bin/env bash
# Liveness, then readiness — the two unauthenticated routes, in the order an
# operator should read them. /health proves only that the process is up;
# /readyz redeems real credentials and is the only evidence the product
# works (docs/architecture.md, "health versus readyz").
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
# -w prints the status: 200 means ready, 503 carries ready:false and a
# reason sentence the runbook maps to a repair.
curl -sS -w '\nHTTP %{http_code}\n' "${BRAMA_URL}/readyz"
