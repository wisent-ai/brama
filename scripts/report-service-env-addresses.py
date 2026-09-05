#!/usr/bin/env python3
"""Print the addresses Brama's service environment file pins.

The unit passes BRAMA_SERVICE_ENV_FILE and the launcher sources it, so a value
there wins over anything the stado config declares. When the gateway keeps
reaching an address no config mentions, this file is where that address lives.

Read-only. Prints a variable's value only when it looks like an address or a
path; everything else is reported as set-or-unset, never quoted.
"""

from __future__ import annotations

import os
import pathlib

path = pathlib.Path(
    os.environ.get("BRAMA_SERVICE_ENV_FILE", str(pathlib.Path.home() / ".config/brama/service.env"))
)

print(f"file: {path}")
if not path.is_file():
    print("state: absent")
    raise SystemExit(0)

for line in path.read_text().splitlines():
    stripped = line.strip()
    if not stripped or stripped.startswith("#") or "=" not in stripped:
        continue
    name, _, raw = stripped.partition("=")
    name = name.removeprefix("export ").strip()
    value = raw.strip().strip('"').strip("'")
    if value.startswith(("http://", "https://", "/", "~", "stado://", "unix:")):
        print(f"{name} = {value}")
    else:
        print(f"{name} = <set>" if value else f"{name} = <empty>")
