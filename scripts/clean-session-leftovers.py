#!/usr/bin/env python3
"""Remove the release directories and binary copies this session left behind.

A superseded copy kept beside its replacement is a second source of truth, and
eleven of them on one host is not a safety net -- the sources are in git and the
live bundle is the only one anything runs.

Two directories are never touched: the one `current` points at, and whichever
one physically holds the trust material the live bundle links to. Removing the
second would take the gateway down, which is the opposite of tidying.
"""
from __future__ import annotations

import json
import os
import shutil
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


live = (SERVICES / "current").resolve()
trust = (live / PLATFORM / "etc" / "brama-skarbiec").resolve()
keep = {live.name}
for parent in trust.parents:
    if parent.parent == SERVICES:
        keep.add(parent.name)
print("live:", live.name)
print("holds trust material:", sorted(keep - {live.name}) or "(the live bundle itself)")

removed = []
for version in sorted(SERVICES.iterdir()):
    if version.is_symlink() or version.name in keep:
        continue
    if MARKER not in provenance_version(version):
        continue
    shutil.rmtree(version)
    removed.append(version.name)
print("removed release directories:", len(removed))
for name in removed:
    print("  ", name)

for candidate in sorted(BIN.glob("*.before-*")):
    candidate.unlink()
    print("removed superseded binary:", candidate.name)
