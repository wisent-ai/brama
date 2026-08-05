#!/bin/sh
set -eu

: "${SKARBIEC_BIN:?SKARBIEC_BIN is required}"
recipient=${BRAMA_SKARBIEC_RECIPIENT_IDENTITY:-brama-rtx@wisent.local}

for item in \
  content-platform-production-model-router \
  echo-model-router \
  oko-model-router \
  weles-model-router \
  weles-keyword-planner-model-router \
  jeden-model-router \
  probierz-model-router \
  wisent-backend-api-model-router \
  wisent-app-model-router \
  growth-tactics-model-router \
  singularity-model-router \
  trading-tools-model-router \
  openenv-model-router \
  trading-autonomy-model-router \
  wisent-trade-agent-model-router \
  wisent-backend-model-router \
  brama-operations-model-router \
  tama-objective-authority-model-router \
  agent:wisent-app \
  echo-agent-auth \
  content-platform-agent-auth \
  oko-model-agent-auth \
  weles-model-agent-auth \
  brama-weles-reauth
do
  "$SKARBIEC_BIN" share "$item" "$recipient"
done
