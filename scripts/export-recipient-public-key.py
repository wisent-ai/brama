#!/usr/bin/env python3
"""Export one recipient's public key from whichever keyring on this host holds it.

Re-encrypting a vault item preserves its existing recipients, so a host that
lacks one recipient's public key cannot share that item at all — gpg answers
`skipped: No public key` and the item stays unreadable for everyone new. The
key is public; moving it between hosts costs nothing and unblocks the re-seal.

Read-only. Prints an armored public key, which is not a secret.

Set BRAMA_RECIPIENT to a fingerprint or uid.
"""
from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
RECIPIENT = os.environ.get("BRAMA_RECIPIENT", "brama-rtx@wisent.local")
KEYRINGS = (
    HOME / ".gnupg",
    HOME / ".stado" / "services" / "brama" / "gnupg",
    Path("/root/.gnupg"),
    Path("/tmp/brama-skarbiec/gnupg"),
)


def gpg_binary() -> str | None:
    for candidate in ("/opt/homebrew/bin/gpg", "/usr/bin/gpg", "/usr/local/bin/gpg"):
        if Path(candidate).exists():
            return candidate
    return shutil.which("gpg")


GPG = gpg_binary()
if not GPG:
    raise SystemExit("gpg is unavailable on this host")

for keyring in KEYRINGS:
    if not keyring.exists():
        continue
    done = subprocess.run(
        [GPG, "--homedir", str(keyring), "--batch", "--armor", "--export", RECIPIENT],
        capture_output=True,
        text=True,
        check=False,
    )
    if "BEGIN PGP PUBLIC KEY BLOCK" in done.stdout:
        print("keyring:", keyring)
        print(done.stdout)
        raise SystemExit()

print("recipient:", RECIPIENT)
print("result: no keyring on this host holds that public key")
