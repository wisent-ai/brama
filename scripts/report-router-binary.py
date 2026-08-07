#!/usr/bin/env python3
"""Say which binary the gateway issues capabilities through, and its version.

`bin/skarbiec-entitlements-router` is the Skarbiec binary under another name, so
"router" and "broker" here are two copies of one product that can drift apart.
When they do, the failure is the newer copy rejecting an argument the older copy
still sends, reported in the authority's words rather than the caller's.

Read-only. Prints paths and versions.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

WAIT_SECONDS = len("....................")

HOME = Path(os.environ.get("HOME", "."))
SERVICE_ENV = Path(
    os.environ.get(
        "BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")
    )
)
CURRENT = HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm"


def version_of(path: Path) -> str:
    if not path.exists():
        return "absent"
    try:
        done = subprocess.run(
            [str(path), "--version"],
            capture_output=True,
            text=True,
            timeout=WAIT_SECONDS,
        )
    except OSError as error:
        return f"unrunnable ({error.strerror})"
    text = (done.stdout or done.stderr).strip()
    return " ".join(text.split()) if text else "silent"


declared = {}
if SERVICE_ENV.exists():
    for line in SERVICE_ENV.read_text().splitlines():
        name, separator, value = line.partition("=")
        if separator and not line.lstrip().startswith("#"):
            declared[name.strip()] = value.strip()

configured = declared.get("ENTITLEMENTS_ROUTER_BIN", "")
print("ENTITLEMENTS_ROUTER_BIN declared:", configured or "(absent)")

targets = [
    ("artifact router", CURRENT / "bin" / "skarbiec-entitlements-router"),
    ("operator skarbiec", HOME / ".stado" / "bin" / "skarbiec"),
]
if configured:
    targets.insert(len(""), ("configured router", Path(configured)))

for label, path in targets:
    print(f"{label}: {path}")
    print("  version:", version_of(path))
