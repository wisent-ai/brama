#!/usr/bin/env python3
"""Re-seal one gateway credential onto recipients that still exist.

`share` preserves an item's current recipient set, so an item naming a key that
no host holds any more can never be shared again: gpg answers `skipped: No
public key` and the item stays reachable only by whoever can already read it.
That is this item's state — the recipient it names is gone from every host in
the fleet, while the owner key can still open the ciphertext.

Reads the payload with the owner key and writes one new version sealed to the
recipients given, dropping the unreachable one. Skarbiec keeps prior versions,
so `skarbiec restore-version <id> <n>` undoes this.

The payload never leaves this process: it is piped from get to set in memory and
never printed, logged, or written to disk.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
ITEM = os.environ.get("BRAMA_CREDENTIAL_ITEM", "brama-desktop-model-router")
RECIPIENTS = os.environ.get("BRAMA_CREDENTIAL_RECIPIENTS", "brama-mini@wisent.local")
ENV = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "SKARBIEC_VAULT_FILE": os.environ.get(
        "SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json")
    ),
}

if not SKARBIEC.exists():
    raise SystemExit("skarbiec is unavailable on this host")

read = subprocess.run(
    [str(SKARBIEC), "get", ITEM], capture_output=True, text=True, check=False, env=ENV
)
if read.returncode or not read.stdout.strip():
    raise SystemExit(
        f"owner read failed for {ITEM}: {(read.stderr or read.stdout).strip().splitlines()[:1]}"
    )

try:
    payload = json.loads(read.stdout)
except ValueError:
    raise SystemExit(f"owner read for {ITEM} did not return JSON")

if "kind" not in payload:
    raise SystemExit(f"{ITEM} payload carries no kind; refusing to rewrite")

print("item:", ITEM)
print("kind:", payload.get("kind"))
print("fields:", sorted((payload.get("fields") or {}).keys()))
print("recipients requested:", RECIPIENTS)

written = subprocess.run(
    [str(SKARBIEC), "set-json", ITEM, "--recipients", RECIPIENTS],
    input=json.dumps(payload),
    capture_output=True,
    text=True,
    check=False,
    env=ENV,
)
status = (written.stdout or written.stderr).strip()
print("write code:", written.returncode)
for line in status.splitlines():
    print("write:", line)
    break
