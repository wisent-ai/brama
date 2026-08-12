#!/usr/bin/env python3
"""Install a delivered Codex auth document into Brama's primary subscription."""

import hashlib
import json
import os
import subprocess
from pathlib import Path

home = Path(os.environ.get("HOME", "."))
# `stado host install-secret` lands a transferred file at $HOME/.stado/<name>,
# and that is the only sanctioned way to move a credential between hosts, so a
# script that knows just the hand-delivered path cannot consume what the fleet's
# own transfer produces.
#
# The host's own live Codex session is the last candidate and the one the pool's
# reauth runner reaches for first: donating a session this machine already holds
# is what it does before driving any login. It is read, never consumed -- a
# delivered file is a copy and is removed once banked, while `~/.codex/auth.json`
# belongs to the CLI on this host and deleting it would sign the machine out.
DELIVERED = [
    home / ".stado/files/codex-auth.json",
    home / ".stado/codex-auth.json",
]
LIVE_SESSION = Path(os.environ.get("CODEX_AUTH_PATH", home / ".codex/auth.json"))
source = next((path for path in [*DELIVERED, LIVE_SESSION] if path.is_file()), DELIVERED[0])
consumable = source in DELIVERED
item = "provider:codex:brama-sub-wisent-app-codex-primary"
service_env = Path(os.environ.get("BRAMA_SERVICE_ENV_FILE", home / ".config/brama/service.env"))
settings = {}
if service_env.is_file():
    for line in service_env.read_text(errors="replace").splitlines():
        name, separator, value = line.partition("=")
        if separator and not line.lstrip().startswith("#"):
            settings[name.strip()] = value.strip().strip('"').strip("'")
vault = Path(settings.get("SKARBIEC_VAULT_FILE", home / ".stado/skarbiec.vault.json"))
skarbiec = home / ".stado/bin/skarbiec"
if not source.is_file():
    raise SystemExit(f"delivered credential is absent: {source}")
auth_text = source.read_text(encoding="utf-8").strip()
auth = json.loads(auth_text)
tokens = auth.get("tokens") if isinstance(auth, dict) else None
access_token = tokens.get("access_token") if isinstance(tokens, dict) else None
account_id = tokens.get("account_id") if isinstance(tokens, dict) else None
if not isinstance(access_token, str) or not access_token:
    raise SystemExit("delivered Codex credential has no tokens.access_token")
if not isinstance(account_id, str) or not account_id:
    raise SystemExit("delivered Codex credential has no tokens.account_id")
document = json.loads(vault.read_text(encoding="utf-8"))
record = (document.get("items") or {}).get(item)
if not isinstance(record, dict):
    raise SystemExit(f"subscription item is absent: {item}")
recipients = record.get("recipients") or []
tags = record.get("tags") or []
if not recipients or not all(isinstance(value, str) for value in recipients):
    raise SystemExit("subscription item has no usable recipients")
canonical = json.dumps(
    {
        "schema": "skarbiec.item.v2",
        "kind": "bundle",
        "fields": {"value": auth_text},
        "context": {"source": "codex-auth.json"},
    }
)
command = [
    str(skarbiec),
    "set-json",
    item,
    "--recipients",
    ",".join(recipients),
]
if tags:
    command.extend(["--tags", ",".join(tags)])
environment = {
    **os.environ,
    **settings,
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "SKARBIEC_VAULT_FILE": str(vault),
}
installed = subprocess.run(
    command,
    input=canonical,
    capture_output=True,
    text=True,
    env=environment,
)
if installed.returncode:
    raise SystemExit("cannot install Codex subscription: " + " ".join(installed.stderr.split()))
if consumable:
    source.unlink()
print(
    "installed:",
    item,
    "access digest:",
    hashlib.sha256(access_token.encode()).hexdigest()[:16],
    "account length:",
    len(account_id),
    "recipients:",
    len(recipients),
)
