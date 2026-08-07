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
if not source_key.exists():
    raise SystemExit(f"no registered key at {source_key}")
if not current_config.exists():
    raise SystemExit(f"current installation has no config directory at {current_config}")

target_key = current_config / KEY
if target_key.exists():
    minted = current_config / f"{KEY}.minted"
    shutil.copy2(target_key, minted)
    print("kept the freshly minted key at:", minted)

shutil.copy2(source_key, target_key)
os.chmod(target_key, stat.S_IRUSR | stat.S_IWUSR)
print("carried forward:", source_key, "->", target_key)
print("bytes:", target_key.stat().st_size)
