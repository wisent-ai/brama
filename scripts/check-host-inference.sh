#!/bin/sh
set -eu

service_env="$HOME/.config/brama/service.env"
release_marker="$HOME/.stado/brama-release-version"
[ -f "$service_env" ] && [ -f "$release_marker" ] || {
  printf '%s\n' 'Brama host runtime is not materialized' >/dev/stderr
  false
}
. "$service_env"
release=$(cat "$release_marker")
router="$HOME/.stado/services/brama/releases/$release/linux-x86_64/bin/skarbiec-entitlements-router"
[ -x "$router" ] || {
  printf '%s\n' "missing Brama entitlement router: $router" >/dev/stderr
  false
}
export GNUPGHOME="$BRAMA_GNUPG_HOME" SKARBIEC_VAULT_FILE
token=$("$router" get wisent-backend-api-model-router | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
[ -n "$token" ] || {
  printf '%s\n' 'empty Brama model-router token' >/dev/stderr
  false
}
if [ -z "${BRAMA_PORT:-}" ]; then
  BRAMA_PORT=$(python3 - "$BRAMA_CONTROL_CONFIG" <<'PY'
import json
import sys

_program, config_path = sys.argv
with open(config_path, encoding="utf-8") as source:
    document = json.load(source)
print(document["services"]["brama"]["port"])
PY
  )
fi
payload='{"model":"wisent-backend/chat/primary","messages":[{"role":"user","content":"Reply exactly: ready"}]}'
curl --fail-with-body --silent --show-error \
  --header "Authorization: Bearer $token" \
  --header 'Content-Type: application/json' \
  --data "$payload" \
  "http://127.0.0.1:${BRAMA_PORT}/v1/chat/completions"
printf '\n'
