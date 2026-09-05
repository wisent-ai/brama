#!/usr/bin/env python3
"""List what a deployment session left behind, separating mine from everyone's.

A superseded copy kept beside its replacement is a second source of truth: the
next operator reads the wrong binary, tests the wrong bundle, or rolls back to
something nobody remembers building. Git is the safety net for source; a host
does not need nine spare release directories to have one that works.

Identifies release directories this session built by the version their
provenance records, marks the one `current` points at, and lists the `before-`
copies left beside promoted binaries. Reports only -- removal is a separate
step, so the list can be read before anything goes.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
PLATFORM = os.environ.get("BRAMA_PLATFORM", "darwin-arm")
MARKER = os.environ.get("SESSION_MARKER", "catalog-capability")
BIN = HOME / ".stado" / "bin"


def provenance_version(version: Path) -> str:
    for record in (version / "provenance.json", version / PLATFORM / "provenance.json"):
        if record.exists():
            try:
                return str(json.loads(record.read_text()).get("version", ""))
            except ValueError:
                return ""
    return ""


live = (SERVICES / "current").resolve().name if (SERVICES / "current").exists() else ""
print("live artifact:", live or "(none)")
mine, others = [], []
for version in sorted(SERVICES.iterdir()) if SERVICES.exists() else []:
    if version.is_symlink():
        if version.name != "current":
            others.append(f"link {version.name}")
        continue
    tag = provenance_version(version)
    if MARKER in tag:
        mine.append((version.name, tag, version.name == live))
print("release directories built this session:", len(mine))
for name, tag, is_live in mine:
    print(f"  {name}  {tag}{'  <- live' if is_live else ''}")

print("superseded copies beside promoted binaries:")
for candidate in sorted(BIN.glob("*.before-*")):
    print("  ", candidate.name)
print("stale current links:")
for entry in others:
    print("  ", entry)
