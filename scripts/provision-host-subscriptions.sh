#!/bin/sh
# Provision this host's four Brama Desktop subscription items with the tag
# discovery contract (`brama:subscription`, `brama:agent:*`, `brama:provider:*`,
# `brama:id:*`). Idempotent. Values are never printed: the report names item ids,
# recipients and tags only.
set -eu

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

BUNDLE="$HOME/.stado/services/brama/current/darwin-arm"
SERVICE_ENV="${BRAMA_SERVICE_ENV_FILE:-$HOME/.config/brama/service.env}"
if [ -f "$SERVICE_ENV" ]; then
  value=$(sed -n 's/^SKARBIEC_VAULT_FILE=//p' "$SERVICE_ENV" | tail -1 | tr -d '"')
  [ -n "$value" ] && export SKARBIEC_VAULT_FILE="$value"
fi
: "${SKARBIEC_VAULT_FILE:=$HOME/.stado/skarbiec.vault.json}"
export SKARBIEC_VAULT_FILE

CLI=""
for candidate in \
  "$HOME/.stado/bin/skarbiec" \
  "$HOME/.local/bin/skarbiec" \
  /opt/homebrew/bin/skarbiec \
  /usr/local/bin/skarbiec \
  "$BUNDLE/bin/skarbiec"
do
  [ -x "$candidate" ] || continue
  if "$candidate" help 2>/dev/null | grep -q '"set-json"'; then CLI="$candidate"; break; fi
done
[ -n "$CLI" ] || { printf 'no skarbiec CLI with set-json on this host\n'; exit 1; }
printf 'cli: %s\nvault: %s\n' "$CLI" "$SKARBIEC_VAULT_FILE"

CLI="$CLI" /usr/bin/python3 <<'PY'
import json, os, subprocess, sys

cli = os.environ["CLI"]
vault_path = os.environ["SKARBIEC_VAULT_FILE"]
environment = {**os.environ}

REFERENCE = "provider:codex:brama-sub-wisent-app-codex-primary"
TARGETS = [
    ("brama-sub-wisent-app-codex-primary", "codex", "codex-reauth-config", ["wisent-app", "lem"]),
    ("brama-sub-wisent-app-codex-secondary", "codex", "codex-reauth-config", ["wisent-app", "lem"]),
    ("brama-sub-wisent-app-claude-primary", "claude-code", "claude-reauth-config", ["wisent-app"]),
    ("brama-sub-wisent-app-kimi-primary", "kimi", "kimi-reauth-config", ["wisent-app"]),
]

with open(vault_path) as handle:
    vault = json.load(handle)
items = vault.get("items") or {}
recipients = (items.get(REFERENCE) or {}).get("recipients") or []
if not recipients:
    recipients = (items.get("codex-reauth-config") or {}).get("recipients") or []
if not recipients:
    print("no recipients could be resolved from the vault envelope")
    sys.exit(1)
print("recipients:", ",".join(recipients))

for subscription_id, provider, source, agents in TARGETS:
    read = subprocess.run([cli, "get", source], capture_output=True, text=True, env=environment)
    if read.returncode:
        print(f"  {subscription_id}: source {source} unreadable: {read.stderr.strip()[:120]}")
        continue
    document = json.loads(read.stdout)
    fields = document.get("fields") or {}
    value = fields.get("value")
    # A non-empty dict is truthy, so the old check passed a reauth *config* --
    # Auth0 and Supabase settings, the recipe for obtaining a credential -- and
    # banked it as the credential itself. The gateway then reported the item as
    # "no value at #value" forever, which reads as a missing subscription rather
    # than a wrong one, and the provisioning run that caused it reported success.
    # The vault stores a secret as a string; anything else is not a credential.
    if set(fields) != {"value"} or not isinstance(value, str) or not value.strip():
        shape = type(value).__name__ if value is not None else "absent"
        print(
            f"  {subscription_id}: source {source} carries {shape} at #value,"
            " not a credential string; refusing to bank it"
        )
        continue
    document["kind"] = "bundle"
    context = document.get("context")
    if not isinstance(context, dict):
        context = {}
        document["context"] = context
    context["source_item"] = source
    context["subscription_owner"] = "wisent-app"
    tags = [
        "brama:subscription",
        f"brama:provider:{provider}",
        f"brama:id:{subscription_id}",
        *[f"brama:agent:{agent}" for agent in agents],
    ]
    item_id = f"provider:{provider}:{subscription_id}"
    written = subprocess.run(
        [cli, "set-json", item_id, "--recipients", ",".join(recipients), "--tags", ",".join(tags)],
        input=json.dumps(document),
        capture_output=True,
        text=True,
        env=environment,
    )
    if written.returncode:
        print(f"  {item_id}: write failed: {written.stderr.strip()[:160]}")
        continue
    print(f"  {item_id}: tags={','.join(tags)}")
PY
