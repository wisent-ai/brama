#!/usr/bin/env python3
"""Name a donation recipient this host's vault actually knows.

Brama refreshes an expired OAuth grant by itself and writes the fresh one back
encrypted to a single recipient. That recipient defaults to `brama-service`,
which is not a key any vault here carries, so every write failed, the refreshed
token survived only in memory, and the stale one came back on the next restart.
From outside this is indistinguishable from a subscription nobody can renew --
and it is why the fleet ran a day with live accounts and dead credentials.

The recipient is chosen from the keys the vault already uses rather than
written here: the gateway key belonging to this host. Nothing is widened; the
gateway is pointed at its own key instead of an absent one.

Idempotent. Prints names only.
"""

from __future__ import annotations

import json
import os
import pathlib
import socket

HOME = pathlib.Path.home()
VAULT = pathlib.Path(os.environ.get("SKARBIEC_VAULT_FILE", HOME / ".stado/skarbiec.vault.json"))
SERVICE_ENV = pathlib.Path(
    os.environ.get("BRAMA_SERVICE_ENV_FILE", HOME / ".config/brama/service.env")
)
KEY = "SKARBIEC_DONATION_RECIPIENT"
PREFIX = "brama-"


def local_part(recipient: str) -> str:
    address, _, _ = recipient.partition("@")
    return address.removeprefix(PREFIX).lower()


document = json.loads(VAULT.read_text())
items = document.get("items") or {}

known = {str(who) for item in items.values() for who in (item.get("recipients") or [])}
gateway_keys = sorted(who for who in known if who.startswith(PREFIX))

print("vault knows:  " + (", ".join(sorted(known)) or "(none)"))
print("gateway keys: " + (", ".join(gateway_keys) or "(none)"))

if not gateway_keys:
    print("this vault carries no gateway key; a donation cannot be encrypted to anyone")
    raise SystemExit(1)

host, _, _ = socket.gethostname().partition(".")
host = host.lower()

matching = [key for key in gateway_keys if local_part(key) and local_part(key) in host]
if len(matching) == 1:
    chosen = matching.pop()
elif len(gateway_keys) == 1:
    chosen = gateway_keys.pop()
else:
    print(f"cannot tell which of {gateway_keys} belongs to {host}; name it explicitly")
    raise SystemExit(1)

print(f"host {host} -> {chosen}")

lines = SERVICE_ENV.read_text().splitlines() if SERVICE_ENV.is_file() else []
present = None
for index, line in enumerate(lines):
    stripped = line.strip().removeprefix("export ").strip()
    name, separator, raw = stripped.partition("=")
    if separator and name.strip() == KEY:
        present = raw.strip().strip('"').strip("'")
        if present != chosen:
            lines[index] = f"{KEY}={chosen}"
        break

print(f"service.env currently: {present or '(unset)'}")
if present == chosen:
    print("already aligned")
    raise SystemExit(0)
if present is None:
    lines.append(f"{KEY}={chosen}")
SERVICE_ENV.write_text("\n".join(lines) + "\n")
print(f"set {KEY}={chosen} in {SERVICE_ENV}")
