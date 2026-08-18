#!/bin/sh
# Regenerate the live installation's Skarbiec policy from the current vault.
# The launcher only reprovisions when the workload registry disagrees with the
# binary, so a policy generated before a subscription was banked keeps serving
# without it. This runs the bundle's own provisioner with --force, which reads
# the vault the fleet holds, and then checks the Kimi rule landed.
set -eu
set -o pipefail 2>/dev/null || true

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"; export PATH
for candidate in /opt/homebrew/bin/node /usr/local/bin/node; do
  [ -x "$candidate" ] && { NODE_BIN="$candidate"; export NODE_BIN; break; }
done
config_dir="${BRAMA_SKARBIEC_CONFIG_DIR:-$HOME/.config/brama/trust}"
bundle=""
for candidate in \
  "$HOME/.stado/services/brama/current/darwin-arm/scripts/provision-skarbiec-trust" \
  "$HOME/.stado/services/brama/current/darwin-arm/scripts/provision-skarbiec-trust.sh" \
  "$HOME/.stado/services/brama/current/darwin-arm/bin/provision-skarbiec-trust"; do
  [ -f "$candidate" ] && { bundle="$candidate"; break; }
done
[ -n "$bundle" ] || { echo "no provision-skarbiec-trust under the serving bundle" >&2; exit 1; }
brama_bin=$(dirname "$bundle")/../bin/brama
[ -x "$brama_bin" ] || { echo "no brama binary beside $bundle" >&2; exit 1; }

BRAMA_SKARBIEC_CONFIG_DIR="$config_dir" \
BRAMA_BIN="$(cd "$(dirname "$brama_bin")" && pwd -P)/$(basename "$brama_bin")" \
BRAMA_WORKLOAD_UID="$(id -u)" \
BRAMA_WORKLOAD_GID="$(id -g)" \
"$bundle" --force

python3 - "$config_dir/policy.json" <<'PY'
import json, os, sys
policy = json.load(open(sys.argv[-1]))
rules = policy.get("roles", {}).get("brama-runtime", [])
kimi = [r.get("resource") for r in rules if "kimi" in str(r.get("resource"))]
subs = [r.get("resource") for r in rules
        if r.get("purpose") == "brama.provider.authenticate"
        and str(r.get("resource", "")).count(":") == 2]
print(f"policy rules: {len(rules)}, subscription rules: {len(subs)}")
for resource in subs:
    print(f"  {resource}")
print(f"kimi rules: {kimi or 'NONE'}")
if not kimi:
    vault_path = os.environ.get("SKARBIEC_VAULT_FILE",
                                os.path.expanduser("~/.stado/skarbiec.vault.json"))
    vault = json.load(open(vault_path))
    item = (vault.get("items") or {}).get(
        "provider:kimi:brama-sub-wisent-app-kimi-primary", {})
    print(f"vault item tags: {item.get('tags')}")
sys.exit(0 if kimi else 1)
PY
