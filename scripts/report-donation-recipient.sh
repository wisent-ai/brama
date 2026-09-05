#!/usr/bin/env bash
# Report whether the gateway can encrypt a refreshed credential to anyone.
#
# Brama refreshes an OAuth grant itself and writes it back with
# `entitlements-router credential-put --recipient <donation recipient>`. When
# that write fails it logs "could not be persisted; using it in memory" and
# drops the reason, so the refreshed token lives until the process restarts and
# the stale one comes back -- which looks exactly like a refresh that never
# happened.
#
# The recipient has to be a key this vault knows. This prints the configured
# recipient, every recipient the vault already uses, and the recipients on the
# subscription items, so the three can be compared.
#
# Read-only.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH
export VAULT_PATH="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"

SERVICE_ENV="$HOME/.config/brama/service.env"

echo "=== configured donation recipient ==="
configured=$(/usr/bin/sed -n 's/^SKARBIEC_DONATION_RECIPIENT=//p' "$SERVICE_ENV" 2>/dev/null | /usr/bin/tail -1 | /usr/bin/tr -d '"')
if [ -n "${configured:-}" ]; then
  echo "service.env: $configured"
else
  echo "service.env: unset -> the binary's default, 'brama-service'"
fi

echo
/usr/bin/python3 <<'PY'
import collections
import json
import os

document = json.load(open(os.environ["VAULT_PATH"]))
items = document.get("items") or {}

seen = collections.Counter()
for item in items.values():
    for who in item.get("recipients") or []:
        seen[who] += len(["one"])

print("=== recipients this vault knows ===")
for who, count in seen.most_common():
    print(f"  {who}   (on {count} items)")

print()
print("=== recipients on the subscription items ===")
for name, item in sorted(items.items()):
    if "brama-sub-" not in name:
        continue
    print(f"  {name}")
    for who in item.get("recipients") or []:
        print(f"      {who}")
PY
