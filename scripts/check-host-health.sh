#!/bin/sh
set -eu

service_env="$HOME/.stado/services/brama/config/service.env"
[ -f "$service_env" ] || {
  printf '%s\n' "missing Brama service environment: $service_env" >/dev/stderr
  false
}
. "$service_env"
: "${BRAMA_PORT:?BRAMA_PORT is required}"
curl --fail --silent --show-error "http://127.0.0.1:${BRAMA_PORT}/health"
printf '\n'
