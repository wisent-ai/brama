#!/usr/bin/env python3
"""Say why one subscription item cannot be read, in the vault's own terms.

`credential unavailable` covers an item sealed to a key this host lacks, an item
whose route names a field it does not carry, and an item that is simply absent.
The gateway reports the same three words for all of them, and the model a user
configured then vanishes from the catalogue with no line saying which.

Read-only: prints recipients, field names, envelope state and the route entry,
never a value.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
GPG = os.environ.get("GPG_BIN", "/opt/homebrew/bin/gpg")
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
ITEM = os.environ.get("SUBSCRIPTION_ITEM", "provider:codex:brama-sub-wisent-app-codex-primary")
ROUTES = Path(
    os.environ.get("SKARBIEC_CAPABILITY_ROUTES_FILE", str(VAULT.with_name("capability-routes.json")))
)

document = json.loads(VAULT.read_text())
entry = (document.get("items") or {}).get(ITEM)
print("item:", ITEM, "present:", entry is not None)
if entry is not None:
    current = entry.get("current")
    # A v2 item holds an object here; a legacy one holds the ciphertext string
    # directly, and `get_item` refuses the whole item with "uses the legacy
    # envelope". Printing the shape says which of the two this is.
    print("  state:", entry.get("state"), "revision:", entry.get("revision"))
    print("  format:", entry.get("format"), "current is:", type(current).__name__)
    print("  recipients:", entry.get("recipients"))
    if isinstance(current, dict):
        print("  has ciphertext:", bool(current.get("ciphertext")))
    else:
        print("  has ciphertext:", bool(current))

print("routes entry:", json.dumps((json.loads(ROUTES.read_text()) if ROUTES.exists() else {}).get(ITEM, "absent")))

opened = subprocess.run(
    [str(SKARBIEC), "get", ITEM],
    capture_output=True,
    text=True,
    env={
        **os.environ,
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "SKARBIEC_VAULT_FILE": str(VAULT),
    },
)
print("read exit:", opened.returncode)
if opened.returncode:
    print("  detail:", " ".join((opened.stderr or "").split()))
else:
    print("  fields:", sorted((json.loads(opened.stdout).get("fields") or {})))

listed = subprocess.run(
    [GPG, "--list-secret-keys", "--with-colons"],
    capture_output=True,
    text=True,
    env={**os.environ, "GNUPGHOME": os.environ.get("BRAMA_GNUPG_HOME", str(HOME / ".gnupg"))},
)
uids = [line.split(":")[len("abcdefdhi")] for line in listed.stdout.splitlines() if line.startswith("uid:")]
print("keyring uids:", uids)
