#!/bin/sh
set -eu

service_env="$HOME/.config/brama/service.env"
[ -f "$service_env" ] || {
  printf '%s\n' "missing Brama service environment: $service_env" >/dev/stderr
  false
}
. "$service_env"
if [ -z "${BRAMA_PORT:-}" ]; then
  BRAMA_PORT=$(python3 - "$BRAMA_CONTROL_CONFIG" <<'PY'
import json
import sys

_program, config_path = sys.argv
with open(config_path, encoding="utf-8") as source:
    document = json.load(source)
port = document["services"]["brama"]["port"]
if isinstance(port, bool) or not isinstance(port, int):
    raise RuntimeError("services.brama.port must be an integer")
print(port)
PY
  )
fi
# Liveness first, because a dead process should be reported as a dead process
# rather than as an unreadable credential.
curl --fail --silent --show-error "http://127.0.0.1:${BRAMA_PORT}/health"
printf '\n'

# Then the question this script is actually asked. `/health` answers ok from a
# gateway whose every capability redemption is refused; on 2026-08-11 it did so
# for a day. `/readyz` redeems one capability per configured provider and fails
# with the providers it could not obtain, so a check that passes here means the
# product works and not merely that the port is open.
if curl --fail --silent --show-error "http://127.0.0.1:${BRAMA_PORT}/readyz"; then
  printf '\n'
else
  status=$?
  printf '%s\n' "readiness failed: the gateway is running but cannot obtain a provider credential" >/dev/stderr
  exit "$status"
fi
