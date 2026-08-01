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
curl --fail --silent --show-error "http://127.0.0.1:${BRAMA_PORT}/health"
printf '\n'
