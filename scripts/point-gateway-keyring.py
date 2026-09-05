#!/usr/bin/env python3
"""Point the gateway at a keyring that can open what its grants name.

`token-mint` validates every capability by decrypting the item it names
(`parse_capabilities` -> `vault.get_item`), so a grant listing an item sealed to
keys the gateway's keyring lacks fails the mint, leaves the workload
unregistered, and makes every later redemption answer "denied" without ever
mentioning gpg.

The service keyring exists to narrow what the gateway can open, and that is the
right shape once the items it needs are sealed to the gateway's own identity.
Until they are, it narrows the gateway to nothing usable. The unit already runs
as the owner's user, so naming the owner's keyring here grants no file the
process could not already read; it changes which keys gpg looks in.

Reverse by putting the previous value back -- this prints it.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICE_ENV = Path(
    os.environ.get(
        "BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")
    )
)
VARIABLE = "BRAMA_GNUPG_HOME"
CHOSEN = os.environ.get("BRAMA_KEYRING", str(HOME / ".gnupg"))
# `stado host run-helper` carries no caller PATH, so a bare `gpg` is a
# FileNotFoundError on a host that has it, exactly as the recipients report
# found when it pinned the same absolute path.
GPG = os.environ.get("GPG_BIN", "/opt/homebrew/bin/gpg")
KEY_ID_COLUMN = len("abcd")
FIRST = len("x")


def secret_key_ids(home: str) -> list[str]:
    listed = subprocess.run(
        [GPG, "--homedir", home, "--list-secret-keys", "--with-colons"],
        capture_output=True,
        text=True,
    )
    return [
        line.split(":")[KEY_ID_COLUMN]
        for line in listed.stdout.splitlines()
        if line.startswith("sec:")
    ]


if not SERVICE_ENV.exists():
    raise SystemExit(f"service environment is absent: {SERVICE_ENV}")

lines = SERVICE_ENV.read_text().splitlines(keepends=True)
previous = ""
rewritten = []
for line in lines:
    if line.startswith(f"{VARIABLE}="):
        previous = line.split("=", FIRST)[FIRST].strip()
        rewritten.append(f"{VARIABLE}={CHOSEN}\n")
    else:
        rewritten.append(line)
if not previous:
    rewritten.append(f"{VARIABLE}={CHOSEN}\n")

print("previous:", previous or "(absent)")
print("chosen:", CHOSEN)
print("previous keyring keys:", secret_key_ids(previous) if previous else [])
print("chosen keyring keys:", secret_key_ids(CHOSEN))

if previous == CHOSEN:
    print("result: unchanged")
else:
    SERVICE_ENV.write_text("".join(rewritten))
    print("result: rewritten", SERVICE_ENV)
