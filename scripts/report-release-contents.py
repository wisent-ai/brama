#!/usr/bin/env python3
"""List what the serving release actually ships.

A redemption denied for every provider, including a capability issued seconds
earlier, means the workload key the gateway proves with is not the one the vault
registered. The launcher registers that key on every start through a helper that
has to be inside the package; when a build ships without it, the registration
silently stops happening and every redemption is refused.

Read-only: it lists the release tree and says whether the registration helper is
present.
"""
from __future__ import annotations

import os
import pathlib

HOME = pathlib.Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
HELPER = "brama-register-workload.py"

current = (SERVICES / "current").resolve()
print("current:", current.name)

entries = sorted(path for path in current.rglob("*") if path.is_file())
print("files:", len(entries))
for path in entries:
    print("  ", path.relative_to(current))

present = [path for path in entries if path.name == HELPER]
print("registration helper:", "present" if present else "ABSENT")
