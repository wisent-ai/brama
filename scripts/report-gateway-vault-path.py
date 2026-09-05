#!/usr/bin/env python3
"""Say which vault the gateway's capability broker actually reads.

`capability-routes.json` must sit beside the vault the broker opens, not beside
the operator's default one. When those differ, a correct routes table written in
the wrong directory produces the same "maps nothing" refusal as no table at all,
which is a long way to travel for a path.

Read-only. Prints paths and variable names, never a value.
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
INTERESTING = ("SKARBIEC_VAULT_FILE", "BRAMA_SKARBIEC_CONFIG_DIR", "BRAMA_GNUPG_HOME")

print("service env:", SERVICE_ENV, "exists:", SERVICE_ENV.exists())
if SERVICE_ENV.exists():
    for line in SERVICE_ENV.read_text().splitlines():
        name, separator, value = line.partition("=")
        if separator and name.strip() in INTERESTING:
            print(f"{name.strip()} = {value.strip()}")

for candidate in (
    HOME / ".stado" / "skarbiec.vault.json",
    Path("/tmp/brama-skarbiec"),
    HOME / ".stado" / "services" / "brama",
):
    print("path:", candidate, "exists:", candidate.exists())
    if candidate.is_dir():
        for child in sorted(candidate.iterdir()):
            print("  entry:", child.name)
