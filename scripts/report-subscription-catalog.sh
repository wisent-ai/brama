#!/usr/bin/env bash
# Print the per-agent model catalogue the gateway was started with.
#
# Every agent's `any` selector answered with codex and nothing else, while the
# vault holds banked Claude and Kimi credentials tagged for those same agents.
# So the gate is not the tags: it is the catalogue the launcher hands the
# gateway, which decides what routes exist at all.
#
# Read-only. Prints providers, route ids and counts; no credential.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH
RUNTIME="${BRAMA_RUNTIME_DIR:-$HOME/.stado/run/brama}"
runtime_candidates="$RUNTIME $HOME/.config/brama/runtime $HOME/.stado/services/brama/current/darwin-arm/var $HOME/.brama"
for candidate in /tmp/brama-skarbiec-*; do
  [ -d "$candidate" ] && runtime_candidates="$runtime_candidates $candidate"
done
echo "runtime dir candidates:"
for candidate in $runtime_candidates; do
  [ -d "$candidate" ] && echo "  $candidate"
done

echo
echo "=== catalogue files found ==="
found=""
for base in $runtime_candidates "$HOME/.config/brama"; do
  for name in subscription-catalog.json subscriptions.json provider-capabilities.json; do
    [ -f "$base/$name" ] || continue
    found=yes
    echo "  $base/$name"
  done
done
[ -n "$found" ] || echo "  (none in the known locations)"

echo
echo "=== providers named in any catalogue ==="
for base in $runtime_candidates "$HOME/.config/brama"; do
  file="$base/subscription-catalog.json"
  [ -f "$file" ] || continue
  echo "  $file"
  /usr/bin/python3 - "$file" <<'PY'
import json
import sys
from collections import Counter

path = sys.argv.pop()
document = json.load(open(path))
rows = (
    document
    if isinstance(document, list)
    else document.get("items") or document.get("models") or document.get("routes") or []
)
if isinstance(rows, dict):
    rows = list(rows.values())
providers = Counter()
for row in rows:
    if isinstance(row, dict):
        providers[str(row.get("provider") or row.get("provider_id") or "?")] += 1
    elif isinstance(row, str) and "/" in row:
        providers[row.split("/", 1)[0]] += 1
print(f"    entries: {len(rows)}")
for provider, count in providers.most_common():
    print(f"    {provider}: {count}")
PY
done

echo
echo "=== Kimi policy rules ==="
for policy in "$HOME/.config/brama/trust/policy.json" "$HOME/.stado/services/brama/current/darwin-arm/config/policy.json"; do
  [ -f "$policy" ] || continue
  echo "  $policy"
  /usr/bin/python3 - "$policy" <<'PY'
import json
import sys

document = json.load(open(sys.argv[-1]))
rules = document.get("roles", {}).get("brama-runtime", [])
for rule in rules:
    resource = str(rule.get("resource") or "")
    if "kimi" in resource:
        print(f"    {rule.get('purpose')}: {resource}")
PY
done
