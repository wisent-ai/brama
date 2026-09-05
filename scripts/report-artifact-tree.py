#!/usr/bin/env python3
"""List what an installed artifact version actually contains.

A release that unpacks one directory level away from where the unit's program
path looks produces no log line at all: launchd cannot exec the launcher, the
unit exits EX_CONFIG, and the failure names nothing. Listing the tree is the
one-command answer.

Read-only.
"""
from __future__ import annotations

import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
VERSION = os.environ.get("ARTIFACT_VERSION", "")

# `stado host run-helper` carries no caller environment, so a helper that reads
# its target from one is a helper that always reports the default. List every
# version instead, two levels deep, which is enough to see whether a release
# unpacked where the unit's program path looks.
DEPTH = len("ab")

for version in sorted(SERVICES.iterdir()) if SERVICES.exists() else []:
    if version.is_symlink():
        print(f"{version.name} -> {os.readlink(version)}")
        continue
    print(version.name)
    for entry in sorted(version.rglob("*")):
        relative = entry.relative_to(version)
        if len(relative.parts) > DEPTH:
            continue
        print(f"  {'dir ' if entry.is_dir() else 'file'} {relative}")
