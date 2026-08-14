#!/bin/sh
# Publish the managed Brama loopback listener through the host's Tailscale Funnel.
set -eu

TAILSCALE=/Applications/Tailscale.app/Contents/MacOS/Tailscale
ORIGIN=http://127.0.0.1:8080
PORT=443

[ -x "$TAILSCALE" ] || {
  printf '%s\n' "Tailscale CLI is not installed" >&2
  exit 1
}

"$TAILSCALE" funnel --bg --https="$PORT" "$ORIGIN" >/dev/null
status=$($TAILSCALE funnel status)
case "$status" in
  *"https://charless-mac-mini."*" (Funnel on)"*"proxy $ORIGIN"*)
    printf '%s\n' "$status"
    ;;
  *)
    printf '%s\n' "$status" >&2
    printf '%s\n' "Brama Funnel postcondition failed" >&2
    exit 1
    ;;
esac
