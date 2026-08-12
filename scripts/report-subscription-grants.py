#!/usr/bin/env python3
"""Report which consumer may read each Brama subscription item.

A subscription can hold a perfectly good credential and still be reported as
"credential unavailable": the gateway reads it through the entitlements router,
which presents a consumer identity, and the vault refuses any read no grant
covers. Grants live on consumer tokens rather than on the items themselves, so
an item can look complete -- value written, tags set, recipients present -- and
still be unreadable by everyone.

Read-only, and never prints a field value or a token: consumer names, actions
and the item#field coordinates they cover.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess

VAULT = pathlib.Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(pathlib.Path.home() / ".stado/skarbiec.vault.json"))
)
CLI = pathlib.Path.home() / ".stado/bin/skarbiec"
WANTED = "brama-sub-"

environment = {**os.environ, "SKARBIEC_VAULT_FILE": str(VAULT)}

if not CLI.is_file():
    print(f"no skarbiec CLI at {CLI}")
    raise SystemExit(1)

result = subprocess.run([str(CLI), "tokens"], capture_output=True, text=True, env=environment)
if result.returncode:
    print(f"tokens failed: {result.stderr.strip()[:200]}")
    raise SystemExit(1)

try:
    tokens = json.loads(result.stdout)
except json.JSONDecodeError as error:
    print(f"tokens returned no JSON: {error}")
    raise SystemExit(1)

if isinstance(tokens, dict):
    tokens = tokens.get("tokens") or []

matched = 0
for token in tokens:
    if not isinstance(token, dict):
        continue
    consumer = token.get("consumer") or token.get("name") or "?"
    covered = []
    for capability in token.get("capabilities") or []:
        if not isinstance(capability, dict):
            continue
        item = str(capability.get("item") or "")
        if WANTED not in item:
            continue
        action = capability.get("action") or "?"
        field = capability.get("field") or "*"
        covered.append(f"{action}:{item}#{field}")
    if covered:
        matched += 1
        print(f"{consumer}")
        for entry in sorted(covered):
            print(f"  {entry}")

if not matched:
    print(f"no consumer token carries any grant over an item containing {WANTED!r}")
