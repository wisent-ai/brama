#!/usr/bin/env python3
"""Give this host's gateway its own vault identity, and only the items it reads.

The gateway keyring here carried an owner key, and the August owner rotation
sealed its deployment credentials past that key. The two repairs that suggest
themselves both widen something: sharing to the owner uid re-opens what the
rotation closed, and pointing the service at the account keyring hands it every
key on the machine. The pattern the fleet already uses one host over is a
per-gateway identity — `brama-rtx@wisent.local` is exactly that — so this
creates the same thing here.

Creates the key inside the gateway's own keyring, registers it as a vault
member, and shares only the items the launcher actually reads. Adds; never
removes a recipient. Reverse with `skarbiec revoke <item> <uid>`.

Read-write on the vault. Prints item ids and status words, never a value.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
GPG = Path("/opt/homebrew/bin/gpg")
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
GATEWAY_KEYRING = HOME / ".stado" / "services" / "brama" / "gnupg"
UID = os.environ.get("BRAMA_GATEWAY_UID", "brama-mini@wisent.local")
ITEMS = (
    "brama-desktop-model-router",
    "brama-operations-model-router",
    "brama-service",
    "brama-token-introspector",
    "brama-weles-reauth",
    "brama-runtime",
    "brama-operations",
)
ENV = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "SKARBIEC_VAULT_FILE": os.environ.get(
        "SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json")
    ),
}


def run(argv: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, capture_output=True, text=True, check=False, env=ENV)


def gateway_has_identity() -> bool:
    done = run([str(GPG), "--homedir", str(GATEWAY_KEYRING), "--batch", "--list-secret-keys", UID])
    return not done.returncode


def first_lines(text: str, limit: int) -> list[str]:
    collected: list[str] = []
    for line in text.strip().splitlines():
        if len(collected) >= limit:
            break
        collected.append(line)
    return collected


if not GPG.exists():
    raise SystemExit("gpg is unavailable on this host")
if not SKARBIEC.exists():
    raise SystemExit("skarbiec is unavailable on this host")

GATEWAY_KEYRING.mkdir(parents=True, exist_ok=True)

if gateway_has_identity():
    print("identity: already present in the gateway keyring")
else:
    recipe = "\n".join(
        (
            "%no-protection",
            "Key-Type: eddsa",
            "Key-Curve: ed25519",
            "Key-Usage: sign",
            "Subkey-Type: ecdh",
            "Subkey-Curve: cv25519",
            "Subkey-Usage: encrypt",
            f"Name-Real: brama gateway {os.uname().nodename}",
            f"Name-Email: {UID}",
            "Expire-Date: 0",
            "%commit",
        )
    )
    with tempfile.NamedTemporaryFile("w", suffix=".batch", delete=False) as handle:
        handle.write(recipe + "\n")
        recipe_path = handle.name
    created = run(
        [str(GPG), "--homedir", str(GATEWAY_KEYRING), "--batch", "--generate-key", recipe_path]
    )
    os.unlink(recipe_path)
    if not gateway_has_identity():
        raise SystemExit(f"key generation failed: {(created.stderr or created.stdout).strip()}")
    print("identity: created in the gateway keyring")

exported = run([str(GPG), "--homedir", str(GATEWAY_KEYRING), "--armor", "--export", UID])
if not exported.stdout.strip():
    raise SystemExit("public key export produced nothing")
with tempfile.NamedTemporaryFile("w", suffix=".asc", delete=False) as handle:
    handle.write(exported.stdout)
    pubkey_path = handle.name

registered = run([str(SKARBIEC), "add-user", UID, "--import", pubkey_path])
# A silent non-zero exit is the failure mode that wasted a round here: report
# the code alongside the text so an empty message is never read as success.
print(
    "add-user: code",
    registered.returncode,
    first_lines(registered.stdout or registered.stderr, len("abc")),
)

for item in ITEMS:
    shared = run([str(SKARBIEC), "share", item, UID])
    payload = (shared.stdout or shared.stderr).strip()
    try:
        report = json.loads(payload)
        state = report.get("status") or ("shared" if report.get("ok") else "unknown")
        print(f"share {item}: code {shared.returncode} {state}")
    except ValueError:
        print(f"share {item}: code {shared.returncode}", first_lines(payload, len("a")))

os.unlink(pubkey_path)
