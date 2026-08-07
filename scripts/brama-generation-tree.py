#!/usr/bin/env python3
"""List what is actually inside the generation the unit is pointed at.

A report that assumes a layout cannot see a layout it did not expect, and a
generation whose files landed one directory deeper than the unit's program path
looks, to launchd, exactly like a missing program: nothing starts and nothing is
logged. This makes no assumption and prints the tree.

Read-only.
"""

import os
import pathlib

DEPTH = len("abc")

home = pathlib.Path.home()
current = (home / ".stado" / "services" / "brama" / "current").resolve()
print(f"=== {current}")

for path in sorted(current.rglob("*")):
    relative = path.relative_to(current)
    if len(relative.parts) > DEPTH:
        continue
    kind = "dir " if path.is_dir() else ("exec" if os.access(path, os.X_OK) else "file")
    print(f"  {kind}  {relative}")
