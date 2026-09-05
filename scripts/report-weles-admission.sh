#!/usr/bin/env bash
# Report whether the Weles admission API answers on this host.
#
# Two records disagree about where it lives: `service directory connect` names
# a fleet address on port 8766, while the directory's own endpoint for this
# host says loopback 8794. Probing the wrong one produced "did not answer" and
# a conclusion that the API was down, so both are asked here, on the machine
# that would serve them.
#
# Read-only.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

for target in "http://127.0.0.1:8794" "http://127.0.0.1:8766" "http://100.120.25.24:8766"; do
  code=$(/usr/bin/curl -s -m 6 -o /dev/null -w '%{http_code}' "$target/" 2>/dev/null || echo 000)
  printf '%s -> HTTP %s%s\n' "$target" "$code" "$([ "$code" = "000" ] && echo '  (nic nie nasluchuje)')"
done

echo
echo "=== kto trzyma te porty ==="
/usr/sbin/lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null | /usr/bin/awk 'NR==1 || /:8794|:8766/ {print $1"  "$2"  "$9}' | head -5

echo
echo "=== co odpowiada 8794 ==="
/usr/bin/curl -s -m 6 "http://127.0.0.1:8794/" 2>/dev/null | head -c 300
echo
