#!/usr/bin/env python3
"""Print what the serving gateway release says about itself.

A rollback or a parallel deployment can move the host onto a build that
predates a fix without anything in the service registry showing it. The release
directory carries a provenance record; this reads the one `current` points at,
so "is my change in what is running" has an answer other than a guess.

Read-only.
"""
from __future__ import annotations

import json
import os
import pathlib

HOME = pathlib.Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"

current = SERVICES / "current"
if not current.exists():
    raise SystemExit(f"no current release under {SERVICES}")

resolved = current.resolve()
print("current:", resolved.name)

records = sorted(resolved.rglob("provenance.json"))
if not records:
    print("no provenance record in the release")
else:
    for record in records:
        try:
            document = json.loads(record.read_text())
        except ValueError as error:
            print(f"{record}: unreadable ({error})")
            continue
        print("record:", record.relative_to(resolved))
        for key in ("version", "commit", "revision", "sourceRevision", "builtAt", "built_at"):
            if key in document:
                print(f"  {key}: {document[key]}")
