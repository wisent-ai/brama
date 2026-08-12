#!/usr/bin/env python3
"""Atomically route Weles trajectories to Codex with local inference fallback."""

from __future__ import annotations

import json
import os
import stat
import tempfile
from pathlib import Path

ROUTES = Path.home() / ".stado" / "inference" / "routes.json"
ALIAS = "weles/agent/primary"
OLD_PRIMARY = "local-openai/chat-primary"
PRIMARY = "codex/gpt-5.6-sol"
FALLBACK = "local-openai/chat-primary"

metadata = ROUTES.lstat()
if not stat.S_ISREG(metadata.st_mode) or ROUTES.is_symlink():
    raise SystemExit(f"inference routes are not a regular file: {ROUTES}")
if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) & 0o077:
    raise SystemExit("inference routes must be owner-only and owned by this workload")

document = json.loads(ROUTES.read_text(encoding="utf-8"))
routes = document.setdefault("routes", {})
fallbacks = document.setdefault("fallbacks", {})
current = routes.get(ALIAS)
if current not in {OLD_PRIMARY, PRIMARY}:
    raise SystemExit(f"refusing to replace unexpected {ALIAS} route: {current!r}")
routes[ALIAS] = PRIMARY
fallbacks[ALIAS] = [FALLBACK]

encoded = json.dumps(document, indent=2, sort_keys=True) + "\n"
fd, staging_name = tempfile.mkstemp(prefix=".routes.", suffix=".tmp", dir=ROUTES.parent)
staging = Path(staging_name)
try:
    os.fchmod(fd, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as output:
        output.write(encoded)
        output.flush()
        os.fsync(output.fileno())
    json.loads(staging.read_text(encoding="utf-8"))
    staging.replace(ROUTES)
finally:
    staging.unlink(missing_ok=True)

print(json.dumps({"status": "updated", "alias": ALIAS, "primary": PRIMARY, "fallback": FALLBACK}))
