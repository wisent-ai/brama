#!/usr/bin/env python3
"""Give the always-on gateway the keyring its launcher decrypts with.

`start-with-skarbiec` reads deployment credentials through the entitlements
router, which shells out to gpg. Without BRAMA_GNUPG_HOME the call lands on the
account's default keyring, which holds none of the recipient keys, and the unit
dies with `gpg failed: encrypted with ECDH key` — an error that looks like a
policy refusal and is a missing environment variable.

Chooses the one candidate keyring that actually holds secret keys, appends it
to the service env file, and reports what it found. Prints variable names and
keyring paths, never a value.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICE_ENV = Path(
    os.environ.get("BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env"))
)
VARIABLE = "BRAMA_GNUPG_HOME"
GPG = Path("/opt/homebrew/bin/gpg")
CANDIDATES = (
    HOME / ".stado" / "services" / "brama" / "gnupg",
    Path("/tmp/brama-skarbiec/gnupg"),
    HOME / ".gnupg",
)


def declared_names(path: Path) -> list[str]:
    if not path.exists():
        return []
    names = []
    for line in path.read_text().splitlines():
        name, separator, _ = line.partition("=")
        if separator and not line.lstrip().startswith("#"):
            names.append(name.strip())
    return names


def holds_secret_keys(home: Path) -> bool:
    if not home.exists() or not GPG.exists():
        return False
    done = subprocess.run(
        [str(GPG), "--homedir", str(home), "--batch", "--list-secret-keys"],
        capture_output=True,
        text=True,
        check=False,
    )
    return "sec" in done.stdout


names = declared_names(SERVICE_ENV)
print("service env:", SERVICE_ENV)
print("declared:", names)

usable = [home for home in CANDIDATES if holds_secret_keys(home)]
for home in CANDIDATES:
    print("candidate:", home, "exists:", home.exists(), "secret keys:", home in usable)

if VARIABLE in names:
    print("result: already declared, nothing changed")
elif len(usable) != len(CANDIDATES[:len(usable)]) or not usable:
    print("result: no candidate keyring holds secret keys, nothing changed")
else:
    chosen = usable[len(usable) - len(usable)]
    with SERVICE_ENV.open("a") as handle:
        handle.write(f"{VARIABLE}={chosen}\n")
    print("result: appended", VARIABLE, "->", chosen)
