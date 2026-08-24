#!/usr/bin/env bash
# Read the subscription pool with fully isolated state. Listing joins the
# deployment's subscription listing to the usage ledger — it contacts no
# provider, redeems no capability, and writes nothing, so it is safe to run
# beside a serving gateway. With fresh temp state it reports the empty pool
# every deployment starts with.
#
# Usage:
#   ./subscription-pool-report.sh
#   BRAMA_BIN=target/debug/brama ./subscription-pool-report.sh
set -euo pipefail

BRAMA_BIN="${BRAMA_BIN:-brama}"
STATE="$(mktemp -d)"
echo "isolated state: ${STATE}" >&2

export HOME="${STATE}"
export BRAMA_STATE_DIR="${STATE}"
export BRAMA_SUBSCRIPTION_USAGE_FILE="${STATE}/subscription-usage.json"
export BRAMA_DONATED_SUBSCRIPTIONS_FILE="${STATE}/donated-subscriptions.json"
export ENTITLEMENTS_ROUTER_BIN="${STATE}/entitlements-router-absent"

echo "== brama subscriptions list"
"${BRAMA_BIN}" subscriptions list

echo
echo "== brama subscriptions list --json"
"${BRAMA_BIN}" subscriptions list --json
