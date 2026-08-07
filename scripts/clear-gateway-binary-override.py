#!/usr/bin/env python3
"""Remove a BRAMA_BIN override so the installed release owns the executable.

The launcher prefers the artifact's own binary and falls back to BRAMA_BIN, and
the workload registry pins the absolute path of the executable allowed to redeem
a capability. An override left in the service environment therefore survives a
release swap and makes the pin disagree with reality -- reported as
`workload registry disagrees on executable_path`, which reads like corrupt trust
material and is a stale environment line.

Prints what it removed; restore by declaring the variable again.
"""
from __future__ import annotations

import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICE_ENV = Path(
    os.environ.get(
        "BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")
    )
)
VARIABLE = "BRAMA_BIN"

if not SERVICE_ENV.exists():
    raise SystemExit(f"service environment is absent: {SERVICE_ENV}")

lines = SERVICE_ENV.read_text().splitlines(keepends=True)
removed = [line.strip() for line in lines if line.startswith(f"{VARIABLE}=")]
if not removed:
    print("no override declared")
else:
    SERVICE_ENV.write_text(
        "".join(line for line in lines if not line.startswith(f"{VARIABLE}="))
    )
    for line in removed:
        print("removed:", line)
