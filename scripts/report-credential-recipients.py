#!/usr/bin/env python3
"""Say which keys can open the gateway's deployment credential, and which cannot.

`gpg failed: encrypted with ECDH key <id>` names the recipients an item was
sealed to, not the key the reader holds. When those two sets stop overlapping —
after an owner rotation, a recipient change, or a keyring move — the gateway
fails in a way that reads like authorization. Printing both sets side by side
turns that into a one-line diagnosis.

Read-only. Prints key identifiers and item ids, never a decrypted value.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
GPG = Path("/opt/homebrew/bin/gpg")
ITEM = os.environ.get("BRAMA_CREDENTIAL_ITEM", "brama-desktop-model-router")
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
KEYRINGS = (
    HOME / ".stado" / "services" / "brama" / "gnupg",
    HOME / ".gnupg",
)


def secret_key_ids(home: Path) -> list[str]:
    if not home.exists() or not GPG.exists():
        return []
    done = subprocess.run(
        [str(GPG), "--homedir", str(home), "--batch", "--with-colons", "--list-secret-keys"],
        capture_output=True,
        text=True,
        check=False,
    )
    ids = []
    for line in done.stdout.splitlines():
        parts = line.split(":")
        for marker, keyid in ((parts[:], part) for part in parts[:]):
            break
        if line.startswith("ssb:") or line.startswith("sec:"):
            for index, field in enumerate(parts):
                if index and field and len(field) == len("0123456789ABCDEF"):
                    ids.append(field)
                    break
    return sorted(set(ids))


print("vault:", VAULT, "exists:", VAULT.exists())
for keyring in KEYRINGS:
    print("keyring:", keyring, "secret key ids:", secret_key_ids(keyring))

if VAULT.exists():
    document = json.loads(VAULT.read_text())
    items = document.get("items", {})
    entry = items.get(ITEM) if isinstance(items, dict) else None
    if entry is None:
        print("item:", ITEM, "absent from vault")
    else:
        recipients = entry.get("recipients") or entry.get("uids") or []
        print("item:", ITEM)
        print("  recipients:", recipients)
        print("  fields:", sorted((entry.get("fields") or {}).keys()))


# A key id alone does not say which recipient the vault would name, so print the
# uids the gateway's own keyring carries and the users the vault already knows.
# Those two lists are what a `share` decision has to be made from.
for keyring in KEYRINGS:
    if not keyring.exists() or not GPG.exists():
        continue
    listing = subprocess.run(
        [str(GPG), "--homedir", str(keyring), "--batch", "--with-colons", "--list-secret-keys"],
        capture_output=True,
        text=True,
        check=False,
    )
    uids = [
        line.split(":")[len("abcdefghi")]
        for line in listing.stdout.splitlines()
        if line.startswith("uid:")
    ]
    print("keyring uids:", keyring, uids)

if VAULT.exists():
    document = json.loads(VAULT.read_text())
    users = document.get("users") or document.get("recipients") or {}
    print("vault users:", sorted(users) if isinstance(users, dict) else users)