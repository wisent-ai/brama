#!/usr/bin/env bash
# Print the subscription manifest this installation actually runs on.
#
# The repository file is a seed for a fresh install. The live one accumulates
# every subscription the host has been given since, and it is what the launcher
# reads to decide which agents may spend which credential and what the signed
# policy was generated from. Reasoning about the repository copy while the host
# runs a longer list is how a subscription looks undeclared when it is not.
#
# Read-only. Prints ids, providers and agent bindings.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

MANIFEST="${BRAMA_SKARBIEC_CONFIG_DIR:-$HOME/.config/brama/trust}/subscriptions.json"
echo "manifest: $MANIFEST"
[ -f "$MANIFEST" ] || { echo "absent"; exit 0; }

/usr/bin/python3 - "$MANIFEST" <<'PY'
import json
import sys

path = sys.argv.pop()
entries = json.load(open(path))
print(f"declared: {len(entries)}")
for entry in entries:
    if not isinstance(entry, dict):
        continue
    agents = entry.get("agents")
    agents = ",".join(agents) if isinstance(agents, list) else "(inferred)"
    print(f"  {entry.get('provider','?'):<12} {entry.get('id','?'):<48} {agents}")
PY
