#!/usr/bin/env python3
"""Carry the gateway's registered workload key into the current installation.

The broker verifies a redemption against the public key the vault holds for
this workload. The config generator mints a fresh keypair every time it runs,
so an update -- which lands the bundle under a new digest directory and
provisions it -- silently replaces the identity the vault knows. The authority
still issues capabilities and the broker then refuses to redeem them, which
reads as a credential that is merely "unavailable".

Until the generator learns to keep an existing key, this puts the registered
one back: the newest previous installation that has a key wins, the current
installation's key is kept beside it as `.minted` so nothing is destroyed, and
the file is copied with its mode.

Prints paths and sizes, never key material.
"""
from __future__ import annotations

import os
import shutil
import stat
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
CURRENT = Path(os.environ.get("BRAMA_BUNDLE", str(SERVICES / "current" / "darwin-arm")))
KEY = "brama-proof.key"
SOURCE_VERSION = os.environ.get("BRAMA_KEY_FROM", "28b4416-cap-macos-b98f875e")

current_config = CURRENT.resolve() / "etc" / "brama-skarbiec"
source_config = SERVICES / SOURCE_VERSION / "darwin-arm" / "etc" / "brama-skarbiec"

print("current:", current_config, "exists:", current_config.exists())
print("source:", source_config, "exists:", source_config.exists())

source_key = source_config / KEY
stable_key = HOME / ".stado" / KEY
print("stable:", stable_key, "exists:", stable_key.exists())

if not source_key.exists():
    raise SystemExit(f"no registered key at {source_key}")
if stable_key.exists():
    print("status: already recorded -- left exactly as it is")
    raise SystemExit()

stable_key.parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(source_key, stable_key)
os.chmod(stable_key, stat.S_IRUSR | stat.S_IWUSR)
print("recorded:", source_key, "->", stable_key)
print("undo: rm", stable_key)
