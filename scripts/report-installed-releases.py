#!/usr/bin/env python3
"""List the gateway artifact versions installed on this host, and which is live.

A launcher older than the broker it drives fails in the broker's words, not its
own: the start log shows the authority rejecting an argument, and nothing says
the shipped launcher stopped sending that argument two releases ago. Knowing
which versions are already on disk decides between moving the service onto one
and building a release to transfer.

Read-only. Prints paths, versions and the resolved link target.
"""
from __future__ import annotations

import datetime
import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = Path(os.environ.get("BRAMA_SERVICES_DIR", str(HOME / ".stado" / "services" / "brama")))

print("services dir:", SERVICES, "exists:", SERVICES.exists())
if SERVICES.exists():
    for child in sorted(SERVICES.iterdir()):
        marker = ""
        if child.is_symlink():
            marker = f" -> {os.readlink(child)}"
        launcher = child / "darwin-arm" / "bin" / "start-with-skarbiec"
        has_launcher = launcher.exists()
        sends_ttl = None
        if has_launcher:
            sends_ttl = "--ttl" in launcher.read_text(errors="replace")
        print(f"  {child.name}{marker} launcher:{has_launcher} sends --ttl:{sends_ttl}")

# A substring test answers whether the flag appears, which is the question only
# until it disagrees with the running log. Print the lines themselves for the
# live artifact, so the launcher's own text settles who sends the argument the
# authority refuses.
live = SERVICES / "current" / "darwin-arm" / "bin" / "start-with-skarbiec"
if live.exists():
    # Which artifact is live can change under a running diagnosis: a second
    # operator moving `current` shows up only as a crash loop that ends by
    # itself. The link's own timestamp is what distinguishes that from a fault
    # in the artifact now serving.
    moved = datetime.datetime.fromtimestamp(
        (SERVICES / "current").lstat().st_mtime, datetime.timezone.utc
    ).isoformat()
    print("current link last moved:", moved)
print("live launcher:", live)
if live.exists():
    text = live.read_text(errors="replace").splitlines()
    start = next(
        (index for index, line in enumerate(text) if line.startswith("def issue(")),
        None,
    )
    if start is None:
        print("  no issue() definition found")
    else:
        window = text[start : start + len("................................")]
        for offset, line in enumerate(window, start=start + len("x")):
            print(f"  {offset}: {line}")
