#!/bin/bash
set -u

# Stado uses sysexits(3) EX_UNAVAILABLE (69) for a transient dependency outage.
# Release-control runs on the same host as Skarbiec, whose supervisor can restore
# it between attempts. Retry only that explicit class; every other failure keeps
# its original exit status and is returned immediately.
attempts=${STADO_RETRY_ATTEMPTS:-5}
delay=${STADO_RETRY_DELAY_SECONDS:-15}

case "$attempts" in
  ''|*[!0-9]*|0) printf 'STADO_RETRY_ATTEMPTS must be a positive integer\n' >&2; exit 64 ;;
esac
case "$delay" in
  ''|*[!0-9]*) printf 'STADO_RETRY_DELAY_SECONDS must be a non-negative integer\n' >&2; exit 64 ;;
esac
[ "$#" -gt 0 ] || { printf 'usage: %s COMMAND [ARG ...]\n' "$0" >&2; exit 64; }

attempt=1
while :; do
  "$@"
  status=$?
  [ "$status" -eq 69 ] || exit "$status"
  [ "$attempt" -lt "$attempts" ] || exit "$status"
  printf 'Stado dependency unavailable; retrying in %ss (%s/%s)\n' "$delay" "$attempt" "$attempts" >&2
  sleep "$delay"
  attempt=$((attempt + 1))
done
