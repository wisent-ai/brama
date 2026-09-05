#!/usr/bin/env python3
"""Show which alias needs a provider capability that was never issued.

The gateway refuses to serve on the first alias whose provider has no issued
capability, and reports that one alias only — so a repair looks like whack-a-mole
until both sides are listed together: the providers the alias table asks for,
and the capabilities the launcher actually issued.

Read-only. Prints alias names, provider ids and capability ids, never a value.
"""
from __future__ import annotations

import json
import os
import re
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
LAUNCHER = Path(
    os.environ.get(
        "BRAMA_LAUNCHER",
        str(HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm" / "bin" / "start-with-skarbiec"),
    )
)
ISSUED = Path("/tmp/brama-skarbiec/provider-capabilities.json")
CATALOG = Path("/tmp/brama-skarbiec/subscription-catalog.json")

print("launcher:", LAUNCHER, "exists:", LAUNCHER.exists())
aliases: dict[str, str] = {}
if LAUNCHER.exists():
    text = LAUNCHER.read_text(errors="replace")
    for block in re.findall(r"BRAMA_MODEL_ALIASES='([^']*)'", text):
        try:
            parsed = json.loads(block)
        except ValueError:
            continue
        if isinstance(parsed, dict):
            aliases.update({str(key): str(value) for key, value in parsed.items()})

for alias, route in sorted(aliases.items()):
    provider = route.split("/")[len("")] if "/" in route else route
    print(f"alias {alias} -> {route} (provider {provider})")

print()
print("issued capabilities:", ISSUED, "exists:", ISSUED.exists())
if ISSUED.exists():
    try:
        issued = json.loads(ISSUED.read_text())
        print("issued keys:", sorted(issued) if isinstance(issued, dict) else issued)
    except ValueError as error:
        print("unreadable:", error)


# Which alias is reported first is HashMap order, so two restarts naming two
# different aliases is one condition, not progress. What separates a stale
# capability from a missing one is when the file was written relative to the
# start that read it, and whether the id still parses as a capability
# reference — so print both, id prefixes only.
import datetime

for path in (ISSUED, Path("/tmp/brama-skarbiec/request-sign-capabilities.json")):
    if not path.exists():
        print("absent:", path)
        continue
    written = datetime.datetime.fromtimestamp(
        path.stat().st_mtime, datetime.timezone.utc
    ).isoformat()
    print("file:", path, "written:", written)
    try:
        content = json.loads(path.read_text())
    except ValueError as error:
        print("  unreadable:", error)
        continue
    if isinstance(content, dict):
        for key, value in sorted(content.items()):
            text = value if isinstance(value, str) else json.dumps(value)
            print(f"  {key}: len={len(text)} prefix={text[:len('abcdefgh')]}")
print("catalog:", CATALOG, "exists:", CATALOG.exists())
if CATALOG.exists():
    try:
        catalog = json.loads(CATALOG.read_text())
        entries = catalog.get("items", catalog)
        names = [entry.get("id") for entry in entries] if isinstance(entries, list) else sorted(entries)
        print("catalog entries:", names)
    except ValueError as error:
        print("unreadable:", error)
