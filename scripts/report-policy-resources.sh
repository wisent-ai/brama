#!/usr/bin/env bash
# Print the subscription resources the signed policy allows.
#
# The launcher drops any vault item whose resource the policy does not name,
# and it drops it with `continue` -- no log line, no counter, nothing. The item
# is banked, tagged and healthy, and simply never enters the catalogue the
# gateway serves from, so every agent sees one provider and no evidence why.
#
# This lists what the policy allows beside what the vault holds, which is the
# comparison that was missing.
#
# Read-only.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH
export TRUST_DIR="${BRAMA_SKARBIEC_CONFIG_DIR:-$HOME/.config/brama/trust}"
export VAULT_PATH="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"

/usr/bin/python3 <<'PY'
import json
import os

policy = json.load(open(os.path.join(os.environ["TRUST_DIR"], "policy.json")))
rules = policy.get("roles", {}).get("brama-runtime", [])
allowed = {
    rule.get("resource")
    for rule in rules
    if isinstance(rule, dict) and rule.get("purpose") == "brama.provider.authenticate"
}
scoped = sorted(name for name in allowed if name and name.count(":") == 2)
bare = sorted(name for name in allowed if name and name.count(":") == 1)

print(f"policy rules for brama-runtime: {len(rules)}")
print(f"  bare provider resources:   {len(bare)}")
print(f"  scoped subscriptions:      {len(scoped)}")
for name in scoped:
    print(f"    {name}")

vault = json.load(open(os.environ["VAULT_PATH"]))
banked = sorted(
    name
    for name, record in (vault.get("items") or {}).items()
    if ":brama-sub-" in name and not record.get("deleted")
)
print()
print("banked subscription items:")
for name in banked:
    mark = "allowed" if name in allowed else "NOT IN POLICY"
    print(f"    {name}  -> {mark}")
PY
