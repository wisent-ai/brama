#!/bin/sh
# Publish Brama's loopback listener through the host's existing Tailscale Funnel.
# The connector owns TLS and this script never makes Brama itself routable.
set -eu

tailscale_bin=${TAILSCALE_BIN:-}
if [ -z "$tailscale_bin" ]; then
    for candidate in \
        /Applications/Tailscale.app/Contents/MacOS/Tailscale \
        /usr/local/bin/tailscale \
        /opt/homebrew/bin/tailscale
    do
        if [ -x "$candidate" ]; then
            tailscale_bin=$candidate
            break
        fi
    done
fi
if [ -z "$tailscale_bin" ]; then
    printf '%s\n' 'Tailscale CLI is unavailable on this host' >&2
    exit 69
fi

"$tailscale_bin" funnel --bg --https=443 http://127.0.0.1:8080
"$tailscale_bin" funnel status --json
