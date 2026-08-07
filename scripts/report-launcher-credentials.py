#!/usr/bin/env python3
"""List the vault items the deployed gateway launcher reads at start.

Sharing "the item that appeared in the error" fixes one start and leaves the
next one to fail on the second item. The launcher names them all; this reads
them out of the deployed script so the recipient set can be made right once.

Read-only. Prints item ids, never a value.
"""
from __future__ import annotations

import os
import re
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
LAUNCHER = Path(
    os.environ.get(
        "BRAMA_LAUNCHER",
        str(HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm" / "bin" / "start-with-skarbiec"),
    )
)
PATTERNS = (
    re.compile(r"(?:secrets|credentials)\s+get[^\n]*?\b([a-z][a-z0-9._:-]{3,})\s*$", re.MULTILINE),
    re.compile(r"item[_-]?id=\"?([a-z][a-z0-9._:-]{3,})"),
    re.compile(r"\b(brama-[a-z0-9._-]+|provider:[a-z0-9._:-]+)\b"),
)

print("launcher:", LAUNCHER, "exists:", LAUNCHER.exists())
if not LAUNCHER.exists():
    raise SystemExit("launcher script is absent")

text = LAUNCHER.read_text(errors="replace")
found: set[str] = set()
for pattern in PATTERNS:
    for match in pattern.findall(text):
        found.add(match)

for item in sorted(found):
    print("referenced:", item)
