#!/usr/bin/env python3
"""Say whether a capability-routes table can be derived, or has to be decided.

Brama refuses to serve when no capability is issued, and points at a
`capability-routes.json` beside the vault that maps each `provider:X` purpose to
one vault coordinate. Its own message says the issuing operator writes it,
because a workload choosing that mapping chooses which credential its purpose
stands for. Whether that is a decision or a transcription depends on one fact:
does each requested provider have exactly one plausible item in this vault.

Read-only. Prints item ids and field names, never a value.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
ROUTES = VAULT.with_name("capability-routes.json")
WANTED = (
    "openai",
    "openrouter",
    "qwen",
    "synthetic",
    "together",
    "venice",
    "xai",
    "zai",
    "codex",
    "anthropic",
    "claude",
    "kimi",
    "moonshot",
    "deepseek",
    "cerebras",
    "fireworks",
    "nvidia",
    "novita",
    "zai-coding",
)

print("routes table:", ROUTES, "exists:", ROUTES.exists())
if not VAULT.exists():
    raise SystemExit("vault is absent on this host")

document = json.loads(VAULT.read_text())
items = document.get("items", {})
ids = sorted(items) if isinstance(items, dict) else []
print("vault items:", len(ids))

for provider in WANTED:
    matches = [item for item in ids if provider in item.lower()]
    print(f"{provider}: {matches}")

# The table's presence says nothing: Brama reports "missing or maps nothing" for
# an empty file too. Print what it holds, and the field names of the provider
# items that do exist here, which is what a route entry has to name.
if ROUTES.exists():
    try:
        table = json.loads(ROUTES.read_text())
        print("routes keys:", sorted(table) if isinstance(table, dict) else table)
    except ValueError as error:
        print("routes unreadable:", error)

for item in ids:
    if item.startswith("provider:"):
        entry = items.get(item) or {}
        print("provider item:", item, "fields:", sorted((entry.get("fields") or {}).keys()))
