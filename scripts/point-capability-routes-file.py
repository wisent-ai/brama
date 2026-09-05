#!/usr/bin/env python3
"""Point the broker at the capability routes table that actually exists.

Brama's own guidance says the table belongs "beside the Skarbiec vault". The
broker disagrees: `routes_path()` honours SKARBIEC_CAPABILITY_ROUTES_FILE and
otherwise looks beside the capability *state* file, which the launcher places in
a per-artifact directory under /tmp. A table written where the message says is
never read, redemption falls back to a challenge coordinate that holds no
ciphertext, and the denial arrives with no reason attached.

Declaring the path explicitly also outlives the artifact: the /tmp directory is
named after the release and disappears with it, so a table left there is lost on
the next deployment.

Reverse by deleting the declared line -- this prints what it wrote.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICE_ENV = Path(
    os.environ.get(
        "BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")
    )
)
VARIABLE = "SKARBIEC_CAPABILITY_ROUTES_FILE"
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
ROUTES = VAULT.with_name("capability-routes.json")

if not SERVICE_ENV.exists():
    raise SystemExit(f"service environment is absent: {SERVICE_ENV}")

print("routes table:", ROUTES, "exists:", ROUTES.exists())
if ROUTES.exists():
    try:
        table = json.loads(ROUTES.read_text())
        print("entries:", sorted(table) if isinstance(table, dict) else table)
    except ValueError as error:
        raise SystemExit(f"routes table unreadable: {error}")

lines = SERVICE_ENV.read_text().splitlines(keepends=True)
declared = [line for line in lines if line.startswith(f"{VARIABLE}=")]
if declared:
    print("already declared:", declared[len(declared) - len(declared)].strip())
else:
    with SERVICE_ENV.open("a") as handle:
        handle.write(f"{VARIABLE}={ROUTES}\n")
    print("declared:", f"{VARIABLE}={ROUTES}")
