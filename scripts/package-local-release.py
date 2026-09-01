#!/usr/bin/env python3
"""Package the current build as a release archive the fleet can install.

`stado host install-release` transfers an immutable archive; this assembles one
from what is already built here, in the layout an installed artifact has on the
host: a platform directory holding `bin` and, at the top, the provenance record
that says which source revision produced it.

The per-installation trust material is deliberately absent, exactly as the
published archives leave it: it names the path, digest and account of the one
binary allowed to redeem a capability, so shipping a copy would describe another
machine. The host re-provisions it against the binary this archive lands.

Writes into target/release/dist, which is build output and not tracked.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import tarfile
from datetime import datetime, timezone
import stat
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXECUTABLE = stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
PLATFORM = os.environ.get("BRAMA_PLATFORM", "darwin-arm")
VERSION = os.environ.get("BRAMA_RELEASE_VERSION", "")
DIST = ROOT / "target" / "release" / "dist"
SKARBIEC_BUILD = Path(
    os.environ.get(
        "SKARBIEC_BUILD", str(ROOT.parent / "skarbiec" / "target" / "release" / "skarbiec")
    )
)

if not VERSION:
    raise SystemExit("set BRAMA_RELEASE_VERSION")

binaries = {
    "brama": ROOT / "target" / "release" / "brama",
    "skarbiec-entitlements-router": SKARBIEC_BUILD,
}
scripts = {
    "start-with-skarbiec": ROOT / "scripts" / "start-with-skarbiec.sh",
    "provision-skarbiec-trust": ROOT / "scripts" / "provision-skarbiec-trust.sh",
}
for name, source in {**binaries, **scripts}.items():
    if not source.exists():
        raise SystemExit(f"missing {name}: {source}")

shutil.rmtree(DIST, ignore_errors=True)
staging = DIST / VERSION
# `stado service update` creates the platform directory itself and unpacks the
# archive inside it, so an archive that carries its own platform level lands as
# `darwin-arm/darwin-arm/bin`, the unit's program path resolves to nothing, and
# launchd exits EX_CONFIG without a line in any log. RELEASE.md describes the
# right shape: `bin/` at the root.
target_bin = staging / "bin"
target_bin.mkdir(parents=True)

# The launcher refuses to start without this beside `bin`, and says so by name.
# RELEASE.md lists it in every published archive; leaving it out produced a
# release that installed cleanly and could not run.
libexec = staging / "libexec"
libexec.mkdir(parents=True)
shutil.copy(ROOT / "scripts" / "generate-skarbiec-config.mjs", libexec / "generate-skarbiec-config.mjs")
# The launcher registers this installation's workload public key on every start
# through this file, and skips the step in silence when it is absent. A package
# without it installs, starts, serves -- and every capability redemption is
# denied on a proof the vault cannot match, which the release workflow avoids by
# shipping it in the same directory.
shutil.copy(
    ROOT / "scripts" / "brama-register-workload.py",
    libexec / "brama-register-workload.py",
)

config = staging / "etc" / "brama-skarbiec"
config.mkdir(parents=True)
shutil.copy(
    ROOT / "scripts" / "skarbiec-recipient-public-keys.asc",
    config / "recipient-public-keys.asc",
)

for name, source in {**binaries, **scripts}.items():
    placed = target_bin / name
    shutil.copy(source, placed)
    placed.chmod(placed.stat().st_mode | EXECUTABLE)

# Copying a linker-signed Mach-O invalidates its signature and macOS kills such
# a process on exec without a message, so each binary is signed where it will be
# unpacked from rather than where it was built.
for name in binaries:
    signed = subprocess.run(
        ["/usr/bin/codesign", "--force", "--sign", "-", str(target_bin / name)],
        capture_output=True,
        text=True,
    )
    if signed.returncode:
        raise SystemExit(f"codesign {name}: {(signed.stderr or signed.stdout).strip()}")

revision = subprocess.run(
    ["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True, text=True
).stdout.strip()
(staging / "provenance.json").write_text(
    json.dumps(
        {
            "product": "brama",
            "version": VERSION,
            "source_revision": revision,
            "platform": PLATFORM,
            "built_at": datetime.now(timezone.utc).isoformat(),
        },
        indent=len("ab"),
        sort_keys=True,
    )
    + "\n"
)
shutil.copy(ROOT / "LICENSE", staging / "LICENSE")

archive = DIST / "brama.tar.gz"
with tarfile.open(archive, "w:gz") as bundle:
    for entry in sorted(staging.rglob("*")):
        bundle.add(entry, arcname=str(entry.relative_to(staging)))

print("archive:", archive, archive.stat().st_size, "bytes")
print("version:", VERSION, "revision:", revision)
print("contents:", sorted(str(path.relative_to(staging)) for path in staging.rglob("*") if path.is_file()))
