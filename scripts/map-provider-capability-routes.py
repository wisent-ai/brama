#!/usr/bin/env python3
"""Map each provider item in this vault to its own capability route.

Brama refuses to serve when no capability is issued and points at
`capability-routes.json`, whose entries say which vault coordinate a purpose
stands for. That mapping is a decision when several items could answer one
purpose — and a transcription when the purpose and the item carry the same
name, which is the case for every `provider:*` item here.

So this writes only identity routes: `provider:X` to the item literally called
`provider:X`, with that item's own single credential field. Anything ambiguous —
no credential field, or several — is reported and skipped rather than guessed.
Existing entries are preserved untouched.

Prints item ids and field names, never a value.
"""
from __future__ import annotations

import json
import os
import stat
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
ROUTES = VAULT.with_name("capability-routes.json")
CREDENTIAL_FIELDS = ("api_key", "token", "key", "secret", "value")
OWNER_ONLY = stat.S_IRUSR | stat.S_IWUSR
INDENT = len("ab")
ENV = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "SKARBIEC_VAULT_FILE": str(VAULT),
}

if not SKARBIEC.exists():
    raise SystemExit("skarbiec is unavailable on this host")

document = json.loads(VAULT.read_text())
items = document.get("items", {})
providers = sorted(item for item in items if item.startswith("provider:"))
table: dict[str, object] = {}
if ROUTES.exists():
    try:
        loaded = json.loads(ROUTES.read_text())
        if isinstance(loaded, dict):
            table = loaded
    except ValueError:
        raise SystemExit("existing routes unreadable; refusing to overwrite")

added = []
for item in providers:
    if item in table:
        print("route present:", item)
        continue
    read = subprocess.run(
        [str(SKARBIEC), "get", item], capture_output=True, text=True, check=False, env=ENV
    )
    if read.returncode or not read.stdout.strip():
        print("unreadable, skipped:", item)
        continue
    try:
        payload = json.loads(read.stdout)
    except ValueError:
        print("non-JSON payload, skipped:", item)
        continue
    fields = sorted((payload.get("fields") or {}).keys())
    candidates = [name for name in CREDENTIAL_FIELDS if name in fields]
    if len(candidates) != len("a"):
        print("ambiguous fields, skipped:", item, fields)
        continue
    chosen = candidates.pop()
    table[item] = {"item": item, "field": chosen}
    added.append((item, chosen))
    print("route added:", item, "->", chosen)

if added:
    ROUTES.write_text(json.dumps(table, indent=INDENT, sort_keys=True) + "\n")
    os.chmod(ROUTES, OWNER_ONLY)
print("routes file:", ROUTES)
print("entries:", sorted(table))
