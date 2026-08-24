#!/bin/sh
# Publish Brama's loopback listener through this host's Tailscale Funnel and
# verify the postcondition.
#
# One idempotent pass: `funnel --bg` with the same arguments is a no-op when
# the rule already matches, so this is safe to run on a schedule forever. The
# rule lives in the Tailscale app's state and has twice been lost without
# anything on the host noticing (a beacon proves a brama process exists, not
# that its public endpoint answers), which is why this is automated instead of
# left as a manual step. Nothing is bound, no secret is read.
#
# Exit 0 only when the funnel answers with the expected proxy line; anything
# else prints what the funnel actually said and exits 1, so a scheduled caller
# records the failure instead of silently passing.
set -eu

TAILSCALE=${TAILSCALE_BIN:-}
if [ -z "$TAILSCALE" ]; then
    for candidate in \
        /Applications/Tailscale.app/Contents/MacOS/Tailscale \
        /usr/local/bin/tailscale \
        /opt/homebrew/bin/tailscale
    do
        if [ -x "$candidate" ]; then
            TAILSCALE=$candidate
            break
        fi
    done
fi
if [ -z "$TAILSCALE" ] || [ ! -x "$TAILSCALE" ]; then
    printf '%s\n' 'brama-funnel: Tailscale CLI is unavailable on this host' >&2
    exit 69
fi

ORIGIN=${BRAMA_FUNNEL_ORIGIN:-http://127.0.0.1:8080}
PORT=${BRAMA_FUNNEL_PORT:-443}

"$TAILSCALE" funnel --bg --https="$PORT" "$ORIGIN" >/dev/null
status=$("$TAILSCALE" funnel status 2>/dev/null || true)
case "$status" in
    *"proxy $ORIGIN"*)
        printf '%s\n' "$status"
        ;;
    *)
        printf '%s\n' "$status" >&2
        printf '%s\n' 'brama-funnel: postcondition failed' >&2
        exit 1
        ;;
esac
