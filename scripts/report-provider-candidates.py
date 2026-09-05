#!/usr/bin/env python3
"""List the readable vault items that could back one provider resource.

A route maps `provider:X` to one vault coordinate. When the item named exactly
`provider:X` exists and carries a single credential field, writing that route is
transcription. When several items could answer, it is a decision, and the
difference decides whether a workload may write the route itself.

Read-only. Prints item ids, envelope shape and field names, never a value.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
NEEDLE = os.environ.get("PROVIDER_NEEDLE", "codex")

document = json.loads(VAULT.read_text())
items = document.get("items") or {}
candidates = sorted(item for item in items if NEEDLE in item.lower())
print("items matching", NEEDLE, ":", len(candidates))
for item_id in candidates:
    entry = items[item_id]
    legacy = not isinstance(entry, dict) or entry.get("format") is None
    print(f"  {item_id}  {'legacy' if legacy else 'v2'}")
    if legacy:
        continue
    opened = subprocess.run(
        [str(SKARBIEC), "get", item_id],
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "SKARBIEC_VAULT_FILE": str(VAULT),
        },
    )
    if opened.returncode:
        print("    unreadable:", " ".join((opened.stderr or "").split()))
        continue
    print("    fields:", sorted((json.loads(opened.stdout).get("fields") or {})))

exact = f"provider:{NEEDLE}"
print()
print("exact item", exact, "present:", exact in items)
