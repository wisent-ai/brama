#!/usr/bin/env python3
"""Provision an installed release's own Skarbiec trust material.

A freshly installed bundle is not self-hosting yet: the launcher only prefers a
bundle's binaries when `etc/brama-skarbiec` sits beside them, so a release that
correctly omits per-installation trust falls back to `/usr/local/bin/brama` and
refuses to start. The bundle ships `bin/provision-skarbiec-trust` for exactly
this, and it writes into its own `etc`, leaving the running version untouched
until `current` is relinked.

Runs the newest installed version that has no trust material yet, and reports
the script's own output. Nothing here decides anything: the bundle pins its own
binary, and the account running it is the account the broker will require.
"""
from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
PLATFORM = os.environ.get("BRAMA_PLATFORM", "darwin-arm")
NODE_CANDIDATES = (
    Path("/opt/homebrew/bin/node"),
    Path("/usr/local/bin/node"),
)

# The trust material proper is `policy.json` and its siblings; the two manifests
# beside them ship with every release and are fleet inputs, not secrets. A
# locally packaged archive that omits them stops the provisioner one step in, so
# they are seeded from the version currently serving.
SHIPPED = ("subscriptions.json", "recipient-public-keys.asc")
live_etc = (SERVICES / "current" / PLATFORM / "etc" / "brama-skarbiec").resolve()

candidates = [
    version
    for version in sorted(SERVICES.iterdir())
    if not version.is_symlink()
    and (version / PLATFORM / "bin" / "provision-skarbiec-trust").exists()
    and not (version / PLATFORM / "etc" / "brama-skarbiec" / "policy.json").exists()
    and (version / PLATFORM / "libexec" / "generate-skarbiec-config.mjs").exists()
]
if not candidates:
    raise SystemExit("every installed release already carries trust material")
node = next((path for path in NODE_CANDIDATES if path.exists()), None)
if node is None:
    raise SystemExit("node is unavailable on this host")

for version in candidates:
    target_etc = version / PLATFORM / "etc" / "brama-skarbiec"
    target_etc.mkdir(parents=True, exist_ok=True)
    for manifest in SHIPPED:
        source = live_etc / manifest
        if source.exists() and not (target_etc / manifest).exists():
            shutil.copy2(source, target_etc / manifest)
            print("  seeded:", manifest)
    script = version / PLATFORM / "bin" / "provision-skarbiec-trust"
    print("provisioning:", version.name)
    done = subprocess.run(
        [str(script)],
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "NODE_BIN": str(node),
        },
    )
    print("  exit:", done.returncode)
    for line in (done.stdout or "").splitlines():
        print("  out:", line)
    for line in (done.stderr or "").splitlines():
        print("  err:", line)
    print(
        "  trust present now:",
        (version / PLATFORM / "etc" / "brama-skarbiec").exists(),
    )
