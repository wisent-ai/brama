#!/bin/sh
# Reconcile the public Funnel's root handler with Brama's endpoint in Stado's
# service directory.
#
# The directory owns placement and the loopback port. Reading it on every pass
# prevents a service move or release-bind change from leaving a hand-written
# origin behind. The root handler is updated with `--set-path /`: port 443 also
# carries Stado object and integration paths, and replacing the whole HTTPS
# handler would erase those shared routes.
#
# This proves only that Funnel publishes the declared origin. Brama API health
# is a separate postcondition for the caller to observe.
set -eu

TAILSCALE=${TAILSCALE_BIN:-$(command -v tailscale || true)}
if [ -z "$TAILSCALE" ]; then
    for candidate in \
        /Applications/Tailscale.app/Contents/MacOS/Tailscale \
        /Applications/Tailscale.app/Contents/MacOS/tailscale \
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

STADO=${STADO_BIN:-$(command -v stado || true)}
if [ -z "$STADO" ]; then
    for candidate in \
        "$HOME/.stado/bin/stado" \
        /Users/charles/.stado/bin/stado \
        /usr/local/bin/stado \
        /opt/homebrew/bin/stado
    do
        if [ -x "$candidate" ]; then
            STADO=$candidate
            break
        fi
    done
fi
if [ -z "$STADO" ] || [ ! -x "$STADO" ]; then
    printf '%s\n' 'brama-funnel: Stado CLI is unavailable on this host' >&2
    exit 69
fi

directory=$("$STADO" service directory endpoint brama --json)
ORIGIN=$(
    printf '%s\n' "$directory" |
        /usr/bin/python3 -c '
import json
import sys
from urllib.parse import urlsplit

entry = json.load(sys.stdin)
url = entry.get("url")
parsed = urlsplit(url) if isinstance(url, str) else None
valid = (
    entry.get("service") == "brama"
    and entry.get("target") == entry.get("active_host")
    and parsed is not None
    and parsed.scheme == "http"
    and parsed.hostname == "127.0.0.1"
    and parsed.port is not None
    and parsed.path in ("", "/")
    and not parsed.query
    and not parsed.fragment
    and parsed.username is None
    and parsed.password is None
)
if not valid:
    raise SystemExit("brama-funnel: Stado did not declare a local Brama HTTP origin on this host")
print(url.rstrip("/"))
'
)

PORT=443
"$TAILSCALE" funnel --bg --https="$PORT" --set-path "/" "$ORIGIN" >/dev/null
status=$("$TAILSCALE" funnel status --json 2>/dev/null || true)
if printf '%s\n' "$status" |
    FUNNEL_ORIGIN="$ORIGIN" FUNNEL_PORT="$PORT" /usr/bin/python3 -c '
import json
import os
import sys

document = json.load(sys.stdin)
origin = os.environ["FUNNEL_ORIGIN"]
port = os.environ["FUNNEL_PORT"]
web = document.get("Web", {})
matches = [
    value.get("Handlers", {}).get("/", {}).get("Proxy")
    for key, value in web.items()
    if key.rsplit(":", 1)[-1] == port
]
allowed = any(
    key.rsplit(":", 1)[-1] == port and value is True
    for key, value in document.get("AllowFunnel", {}).items()
)
if matches != [origin] or not allowed:
    raise SystemExit(1)
'
then
    printf '%s\n' "brama-funnel: published declared origin $ORIGIN at HTTPS $PORT path /"
else
    printf '%s\n' "$status" >&2
    printf '%s\n' 'brama-funnel: declared-origin postcondition failed' >&2
    exit 1
fi
