#!/usr/bin/env python3
"""Count and name the vault items still in the pre-v2 envelope.

`get_item` refuses a legacy item outright -- "item uses the legacy envelope
(run migrate-v2)" -- so every credential still in that shape is invisible to the
gateway: the launcher skips its client identity, native model discovery drops
the provider, and a model a user configured disappears from the catalogue with
"not in the Brama catalog". Nothing in that chain says the word envelope.

The remedy is one documented command over the whole store, and the decision to
run it belongs to whoever owns the store, so this only measures: how many items
are affected, which ones, and whether any of them back a subscription or a
provider the gateway routes to.

Read-only. Prints item ids and envelope shape, never a value.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)

document = json.loads(VAULT.read_text())
items = document.get("items") or {}
legacy = []
for item_id, entry in items.items():
    if not isinstance(entry, dict):
        continue
    # A v2 item records `format: 2` and holds an object under `current`; a
    # legacy one has neither.
    if entry.get("format") is None or not isinstance(entry.get("current"), dict):
        legacy.append(item_id)

print("vault:", VAULT)
print("items:", len(items), "legacy:", len(legacy))
for item_id in sorted(legacy):
    kind = "subscription" if item_id.count(":") > len(":") - len(":") + len("x") else "item"
    print(f"  {kind}: {item_id}")
if legacy:
    print()
    print("Every one of these is unreadable to the gateway until the store is")
    print("migrated; the command that does it is `skarbiec migrate-v2`, and it")
    print("rewrites each item's envelope, which is why it is the owner's to run.")
