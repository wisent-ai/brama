#!/usr/bin/env python3
"""Remove trust directories that hold manifests but no trust material.

The launcher treats a bundle as self-hosting when `etc/brama-skarbiec` exists
beside `bin`, so a directory containing only the shipped manifests makes a
release claim components it cannot authenticate with, and the gateway starts
from that bundle and refuses every redemption. A half-seeded directory is worse
than no directory.

Removes only directories that lack `policy.json`, which is the file that says
provisioning finished. Prints every path it touches.
"""
from __future__ import annotations

import os
import shutil
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
PLATFORM = os.environ.get("BRAMA_PLATFORM", "darwin-arm")

for version in sorted(SERVICES.iterdir()) if SERVICES.exists() else []:
    if version.is_symlink():
        continue
    trust = version / PLATFORM / "etc" / "brama-skarbiec"
    if not trust.is_dir():
        continue
    if (trust / "policy.json").exists():
        continue
    entries = sorted(path.name for path in trust.iterdir())
    shutil.rmtree(trust)
    print("removed:", trust, "held:", entries)
print("done")
