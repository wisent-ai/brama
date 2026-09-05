#!/usr/bin/env python3
"""Atomically point the declared local model deployment at this host's listener."""
from __future__ import annotations

import json
import os
import stat
import tempfile
from pathlib import Path

ROUTES = Path.home() / ".stado" / "inference" / "routes.json"
DEPLOYMENT = "chat-primary"
EXPECTED = {("100.126.122.108", 8001), ("127.0.0.1", 8001)}
TARGET = ("127.0.0.1", 8001)

metadata = ROUTES.lstat()
if not stat.S_ISREG(metadata.st_mode) or ROUTES.is_symlink():
    raise SystemExit(f"inference routes are not a regular file: {ROUTES}")
if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) & 0o077:
    raise SystemExit("inference routes must be owner-only and owned by this workload")

document = json.loads(ROUTES.read_text(encoding="utf-8"))
deployments = document.get("deployments")
if not isinstance(deployments, list):
    raise SystemExit("inference routes deployments must be a list")
matches = [entry for entry in deployments if isinstance(entry, dict) and entry.get("name") == DEPLOYMENT]
if len(matches) != 1:
    raise SystemExit(f"expected one {DEPLOYMENT!r} deployment, found {len(matches)}")
endpoint = matches[0].get("endpoint")
if not isinstance(endpoint, dict):
    raise SystemExit(f"{DEPLOYMENT!r} has no endpoint object")
current = (endpoint.get("host"), endpoint.get("port"))
if current not in EXPECTED:
    raise SystemExit(f"refusing unexpected {DEPLOYMENT!r} endpoint: {current!r}")
endpoint["host"], endpoint["port"] = TARGET

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

print(json.dumps({
    "status": "unchanged" if current == TARGET else "updated",
    "deployment": DEPLOYMENT,
    "previous": {"host": current[0], "port": current[1]},
    "endpoint": {"host": TARGET[0], "port": TARGET[1]},
}))
