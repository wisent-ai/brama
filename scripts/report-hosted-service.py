#!/usr/bin/env python3
"""Print bounded Brama service logs without exposing its environment."""

from pathlib import Path

LOG_DIR = Path.home() / ".stado" / "logs"
for name in ("brama-always-on.out", "brama-always-on.err"):
    path = LOG_DIR / name
    print(f"== {path} ==")
    if not path.is_file():
        print("missing")
        continue
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    print("\n".join(lines[-80:]))
