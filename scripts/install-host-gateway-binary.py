#!/usr/bin/env python3
"""Put a delivered gateway build in the operator bin and name it in the service.

`stado host install-binary` verifies a build by running its CLI, and the gateway
has no such CLI, so the transfer channel is `install-file` and the placing is
here. Two things this does that a copy does not:

Copying a linker-signed Mach-O invalidates its signature, and macOS then kills
the process on exec with no message at all, so the binary is re-signed at the
path it will run from.

The service environment names the executable through BRAMA_BIN, so pointing that
at the operator bin leaves the installed release archive untouched and reverses
by deleting one line.
"""
from __future__ import annotations

import os
import shutil
import stat
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
DELIVERED = HOME / ".stado" / "files" / os.environ.get("GATEWAY_FILE", "brama-catalog-fix")
TARGET = HOME / ".stado" / "bin" / "brama"
SERVICE_ENV = Path(
    os.environ.get(
        "BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")
    )
)
VARIABLE = "BRAMA_BIN"
OWNER_EXECUTABLE = stat.S_IRWXU

if not DELIVERED.exists():
    raise SystemExit(f"delivered build is absent: {DELIVERED}")

if TARGET.exists():
    kept = TARGET.with_name(TARGET.name + ".before-catalog-fix")
    shutil.copy2(TARGET, kept)
    print("kept:", kept)

shutil.copy(DELIVERED, TARGET)
os.chmod(TARGET, OWNER_EXECUTABLE)
signed = subprocess.run(
    ["/usr/bin/codesign", "--force", "--sign", "-", str(TARGET)],
    capture_output=True,
    text=True,
)
if signed.returncode:
    raise SystemExit(f"codesign failed: {(signed.stderr or signed.stdout).strip()}")
print("installed:", TARGET)

lines = SERVICE_ENV.read_text().splitlines(keepends=True) if SERVICE_ENV.exists() else []
previous = [line for line in lines if line.startswith(f"{VARIABLE}=")]
rewritten = [line for line in lines if not line.startswith(f"{VARIABLE}=")]
rewritten.append(f"{VARIABLE}={TARGET}\n")
SERVICE_ENV.write_text("".join(rewritten))
print("previous:", previous[len(previous) - len(previous)].strip() if previous else "(absent)")
print("declared:", f"{VARIABLE}={TARGET}")
