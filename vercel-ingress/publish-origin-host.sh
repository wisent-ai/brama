#!/bin/sh
# Publish this host's loopback Brama as the public origin that vercel.json
# rewrites to, and report what is actually being served.
#
#   stado host install-helper <target> vercel-ingress/publish-origin-host.sh brama-publish-origin.sh
#   stado host run-helper <target> brama-publish-origin.sh
#
# Why this exists: vercel.json rewrites brama.wisent.com to this host's
# Tailscale hostname, which is public only while Tailscale Funnel terminates
# TLS for it. On 2026-08-17 a power cut took the funnel down; Vercel then
# answered every Brama request with ROUTER_EXTERNAL_TARGET_HANDSHAKE_ERROR and
# X-Vercel-Error: DNS_HOSTNAME_EMPTY, and every model call in the fleet failed.
# Re-serving is idempotent: an already-correct funnel is left alone.
set -eu

port="${BRAMA_ORIGIN_PORT:-8080}"
tailscale=/Applications/Tailscale.app/Contents/MacOS/Tailscale
[ -x "$tailscale" ] || tailscale=$(command -v tailscale || true)
[ -n "$tailscale" ] && [ -x "$tailscale" ] || { echo "tailscale CLI not found" >&2; exit 1; }

printf '== brama listener ==\n'
if /usr/sbin/lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "brama is listening on 127.0.0.1:$port"
else
    echo "nothing is listening on 127.0.0.1:$port; refusing to publish an empty origin" >&2
    exit 1
fi

printf '\n== funnel state before ==\n'
"$tailscale" funnel status 2>&1 || true

if ! "$tailscale" funnel status 2>/dev/null | grep -q "127.0.0.1:$port"; then
    printf '\n== re-serving ==\n'
    "$tailscale" funnel --bg --https=443 "http://127.0.0.1:$port"
fi

printf '\n== funnel state after ==\n'
"$tailscale" funnel status 2>&1 || true


printf '\n== https certificate ==\n'
# Funnel terminates TLS with the node's Let's Encrypt certificate. Without a
# provisioned certificate the relay accepts the TCP connection and drops it,
# which is exactly the SSL EOF a client sees and the handshake error Vercel
# reports.
name=$("$tailscale" status --json | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')
cert_dir=$(/usr/bin/mktemp -d /tmp/brama-origin-cert.XXXXXX)
"$tailscale" cert --cert-file "$cert_dir/cert.pem" --key-file "$cert_dir/key.pem" "$name" 2>&1 | tail -5 || true
if [ -s "$cert_dir/cert.pem" ]; then
    /usr/bin/openssl x509 -in "$cert_dir/cert.pem" -noout -subject -enddate 2>&1 || true
fi
rm -rf "$cert_dir"
printf '\n== funnel capability ==\n'
"$tailscale" status --json | /usr/bin/python3 -c 'import json,sys
state = json.load(sys.stdin)
caps = state["Self"].get("CapMap") or {}
print("funnel attr:", "funnel" in " ".join(caps) or [k for k in caps if "funnel" in k.lower()])
print("cap keys:", sorted(caps)[:12])
print("tailscale version:", state.get("Version"))'

printf '\n== local origin check ==\n'
/usr/bin/curl -sS -o /dev/null -w 'loopback /healthz -> %{http_code}\n' \
    "http://127.0.0.1:$port/healthz" || true
printf 'public name: %s\n' "$name"
/usr/bin/curl -sS -o /dev/null -w 'public /healthz -> %{http_code}\n' --max-time 20 \
    "https://$name/healthz" || true
