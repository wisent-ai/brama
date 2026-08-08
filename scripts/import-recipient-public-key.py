#!/usr/bin/env python3
"""Import a delivered recipient public key into the gateway's keyring.

An item is re-encrypted to every recipient it names, so a host missing one
recipient's *public* key cannot rewrite that item at all -- gpg answers
"skipped: No public key" and the migration stops on that item alone. The fix is
the public half, which is not a secret and grants nothing: it lets this host
encrypt *to* that recipient, never decrypt for them.

Reads the armoured key from the operator's files directory and imports it into
the keyring the gateway actually uses. Prints fingerprints and uids only.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
GPG = os.environ.get("GPG_BIN", "/opt/homebrew/bin/gpg")
DELIVERED = HOME / ".stado" / "files" / os.environ.get("RECIPIENT_KEY_FILE", "brama-rtx-public.asc")
SERVICE_ENV = HOME / ".config" / "brama" / "service.env"


def gateway_keyring() -> str:
    if SERVICE_ENV.exists():
        for line in SERVICE_ENV.read_text().splitlines():
            name, separator, value = line.partition("=")
            if separator and name.strip() == "BRAMA_GNUPG_HOME":
                return value.strip()
    return str(HOME / ".gnupg")


if not DELIVERED.exists():
    raise SystemExit(f"no delivered key at {DELIVERED}")

keyring = gateway_keyring()
print("keyring:", keyring)
imported = subprocess.run(
    [GPG, "--homedir", keyring, "--import", str(DELIVERED)],
    capture_output=True,
    text=True,
)
print("import exit:", imported.returncode)
for line in (imported.stderr or "").splitlines():
    print("  ", line)

listed = subprocess.run(
    [GPG, "--homedir", keyring, "--list-keys", "--with-colons"],
    capture_output=True,
    text=True,
)
uids = [
    line.split(":")[len("abcdefdhi")]
    for line in listed.stdout.splitlines()
    if line.startswith("uid:")
]
print("keyring uids:", uids)
