#!/usr/bin/env python3
"""List the proof keys in the serving trust directory and when each was written.

The vault registers one workload public key per agent, and the gateway proves a
redemption with the private half in `brama-proof.key`. Regenerating that file
without registering the new public half leaves every redemption failing on the
proof, which reads as a denied capability and not as a key that moved.

Recovering means putting back the key the registration still names, so this
lists what the directory holds and its timestamps. Prints names, sizes and
fingerprints, never key material.
"""
from __future__ import annotations

import datetime
import hashlib
import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
PLATFORM = os.environ.get("BRAMA_PLATFORM", "darwin-arm")

trust = (SERVICES / "current" / PLATFORM / "etc" / "brama-skarbiec").resolve()
print("trust directory:", trust)
for entry in sorted(trust.iterdir()):
    if not entry.is_file():
        continue
    info = entry.stat()
    written = datetime.datetime.fromtimestamp(
        info.st_mtime, datetime.timezone.utc
    ).isoformat()
    marker = ""
    if "proof" in entry.name or entry.name.endswith(".key"):
        marker = " fingerprint=" + hashlib.sha256(entry.read_bytes()).hexdigest()[: len("abcdefgh")]
    print(f"  {entry.name} {info.st_size}B {written}{marker}")
