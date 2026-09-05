#!/usr/bin/env python3
"""Turn a delivered bundle into an installed generation, and nothing more.

The bundle arrives through `stado host install-file`, which verifies its digest
on the way in. This unpacks it into a generation named after that digest, so the
name says exactly what is in it and installing the same bundle twice cannot
produce two different directories.

It deliberately stops there. Provisioning the installation's identity and
repointing the service manager are `brama-activate-generation`'s job, in that
order, because the other order is what leaves a gateway answering `/health` and
serving nothing.
"""

import hashlib
import pathlib
import shutil
import tarfile

BUNDLE_NAME = "brama-bundle.tar.gz"
NAME_LENGTH = len("0123456789ab")
EXECUTABLES = (
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
    "bin/provision-skarbiec-trust",
)

home = pathlib.Path.home()
delivered = home / ".stado" / "files" / BUNDLE_NAME
services = home / ".stado" / "services" / "brama"

if not delivered.is_file():
    raise SystemExit(f"no delivered bundle at {delivered}")

digest = hashlib.sha256(delivered.read_bytes()).hexdigest()
print(f"bundle sha256 {digest}")

generation = services / f"local-{digest[:NAME_LENGTH]}"
if generation.exists():
    shutil.rmtree(generation)
generation.mkdir(parents=True)

with tarfile.open(delivered) as archive:
    unsafe = [
        name
        for name in archive.getnames()
        if name.startswith("/") or ".." in pathlib.Path(name).parts
    ]
    if unsafe:
        raise SystemExit(f"bundle contains unsafe paths: {', '.join(unsafe)}")
    archive.extractall(generation)

root = generation / "darwin-arm"
reference = root / "bin" / "brama"
if not reference.is_file():
    raise SystemExit("bundle did not contain darwin-arm/bin/brama")

for relative in EXECUTABLES:
    target = root / relative
    target.chmod(target.stat().st_mode | reference.stat().st_mode)
    print(f"  {relative}")

print(f"installed {generation}")
print("next: brama-activate-generation")
