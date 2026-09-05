#!/usr/bin/env python3
"""Print the tail of the gateway's own error stream.

`service logs` shows the unit's stdout file. A gateway that dies during
startup usually says why on stderr, in a different file, and without it the
failure reads as silence: the last line is whatever the launcher printed, and
the reason sits in a file nobody looked at.

Read-only. Prints log lines, which are diagnostics, never a credential value --
the launcher is careful to keep secrets out of them.
"""
from __future__ import annotations

import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
LOGS = HOME / ".stado" / "logs"
TAIL_LINES = len("x" * len("twenty lines is enough"))


def tail(path: Path) -> None:
    print("==", path, "exists:", path.exists())
    if not path.exists():
        return
    for line in path.read_text(errors="replace").splitlines()[-TAIL_LINES:]:
        print("  ", line)


for name in ("brama-always-on.err", "brama-always-on.out", "com.wisent.always-on.brama.err"):
    tail(LOGS / name)
