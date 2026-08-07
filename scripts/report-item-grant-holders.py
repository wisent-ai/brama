#!/usr/bin/env python3
"""Say which consumers may read one item field here, and which have a token file.

"No consumer is authorized" is a claim, and asking one consumer does not prove
it. The vault's own grant table lists every holder; the operator bin lists which
of them this host can actually present. Both together decide whether a value can
move through the audited channel or whether a grant has to be minted first.

Read-only. Prints consumer names, actions and coordinates, never a value.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = HOME / ".stado" / "skarbiec.vault.json"
ITEM = os.environ.get("GRANT_ITEM", "agent:wisent-app")

listed = subprocess.run(
    [str(SKARBIEC), "tokens"],
    capture_output=True,
    text=True,
    env={
        **os.environ,
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "SKARBIEC_VAULT_FILE": str(VAULT),
    },
)
if listed.returncode:
    raise SystemExit(f"tokens listing failed: {' '.join((listed.stderr or '').split())}")

grants = json.loads(listed.stdout)
print("item:", ITEM)
holders = []
for grant in grants if isinstance(grants, list) else []:
    consumer = grant.get("consumer", "")
    for capability in grant.get("capabilities", []) or []:
        if capability.get("item") != ITEM:
            continue
        holders.append((consumer, capability.get("action", ""), capability.get("field", "")))
for consumer, action, field in sorted(set(holders)):
    print(f"  {consumer}: {action} {ITEM}#{field}")
if not holders:
    print("  no grant names this item")

print("token files present here:")
for path in sorted((HOME / ".stado").glob("*-skarbiec-token")):
    print("  ", path.name)
