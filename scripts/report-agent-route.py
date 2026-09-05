#!/usr/bin/env python3
"""Print the capability route for one resource and the fields its item carries.

A route names the vault coordinate a purpose stands for. When it names the wrong
field of the right item, every redemption succeeds and returns the wrong string:
the gateway verifies a signature against a value the caller never signed with,
and answers 401 as if the credential were forged.

Read-only. Prints coordinates and field names, never a value.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
ROUTES = Path(os.environ.get("SKARBIEC_CAPABILITY_ROUTES_FILE", str(VAULT.with_name("capability-routes.json"))))
RESOURCE = os.environ.get("ROUTE_RESOURCE", "agent:wisent-app")
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"

print("routes file:", ROUTES, "exists:", ROUTES.exists())
if ROUTES.exists():
    table = json.loads(ROUTES.read_text())
    print("entry:", json.dumps(table.get(RESOURCE, "absent"), sort_keys=True))
    print("all entries:", sorted(table))

opened = subprocess.run(
    [str(SKARBIEC), "get", RESOURCE],
    capture_output=True,
    text=True,
    env={
        **os.environ,
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "SKARBIEC_VAULT_FILE": str(VAULT),
    },
)
if opened.returncode:
    print("item unreadable:", " ".join((opened.stderr or "").split()))
else:
    fields = json.loads(opened.stdout).get("fields") or {}
    print("item fields:", sorted(fields))
