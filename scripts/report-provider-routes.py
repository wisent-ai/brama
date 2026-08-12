#!/usr/bin/env python3
"""Name the vault coordinate each provider capability is served from.

The gateway refuses a subscription with `provider_authentication`, and the next
question is always the same: which item and field carries that credential, so a
refreshed one can be put where the router will actually read it. The routes file
answers it; printing it beats guessing at item names.

Read-only, and it prints coordinates only -- never a secret.
"""

import json
import os
import pathlib
import sys

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
ROUTES = pathlib.Path(
    os.environ.get("SKARBIEC_CAPABILITY_ROUTES_FILE", HOME / ".stado" / "capability-routes.json")
)


def main():
    if not ROUTES.is_file():
        raise SystemExit(f"no routes file at {ROUTES}")
    document = json.loads(ROUTES.read_text(encoding="utf-8"))
    table = document.get("routes", document)
    print(f"routes    {ROUTES}")
    for resource in sorted(table):
        entry = table[resource]
        if isinstance(entry, dict):
            item = entry.get("item") or entry.get("id") or "?"
            field = entry.get("field") or entry.get("Field") or "?"
            print(f"{resource:<34} item {item}  field {field}")
        else:
            print(f"{resource:<34} {entry}")
    return NONE


sys.exit(main())
