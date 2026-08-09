#!/usr/bin/env python3
"""Provision one Brama caller without exposing either long-lived credential.

Run on the Skarbiec owner host. The request-signing secret is generated there,
written to the owner vault over stdin, and never printed or placed in argv. The
caller's workload public key authorizes one-time acquisition of exactly the
model-router bearer and its own signing secret.
"""

import json
import os
from pathlib import Path
import secrets
import subprocess
import sys


home = Path.home()
if len(sys.argv) == 1:
    consumer = "lem"
    public_key = home / ".stado" / "files" / "lem-workload-ed25519.pub.pem"
elif len(sys.argv) == 3:
    consumer = sys.argv[1]
    public_key = Path(sys.argv[2]).expanduser().resolve()
else:
    raise SystemExit("usage: provision-model-consumer.py [<consumer> <workload-public-key.pem>]")

if not consumer or any(character not in "abcdefghijklmnopqrstuvwxyz0123456789-_" for character in consumer):
    raise SystemExit("consumer must be one lowercase identifier")
if not public_key.is_file():
    raise SystemExit(f"workload public key is missing: {public_key}")

settings = {}
env_file = home / ".config" / "brama" / "service.env"
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

environment = {**os.environ, **settings}
environment.setdefault("SKARBIEC_VAULT_FILE", str(home / ".stado" / "skarbiec.vault.json"))
skarbiec = home / ".stado" / "bin" / "skarbiec"
if not skarbiec.is_file():
    raise SystemExit(f"Skarbiec binary is missing: {skarbiec}")


def invoke(arguments, *, payload=None):
    result = subprocess.run(
        [str(skarbiec), *arguments],
        input=payload,
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )
    if result.returncode:
        detail = (result.stderr.strip() or result.stdout.strip()).replace("\n", " ")
        raise SystemExit(f"skarbiec {' '.join(arguments)} refused: {detail}")
    return result.stdout


items = json.loads(invoke(["list"]))
auth_item = f"{consumer}-agent-auth"
model_item = f"{consumer}-model-router"
active_ids = {
    item.get("id")
    for item in items
    if isinstance(item, dict) and not item.get("deleted", False)
}
if model_item not in active_ids:
    raise SystemExit(f"required model-router item is missing: {model_item}")

if auth_item not in active_ids:
    payload = json.dumps({
        "schema": "skarbiec.item.v2",
        "kind": "internal-authority",
        "fields": {
            "id": consumer,
            "agent_auth_secret": secrets.token_urlsafe(48),
        },
        "context": {},
    }, separators=(",", ":"))
    invoke(["set-json", auth_item, "--type", "internal-authority"], payload=payload)
    print(f"created {auth_item}")
else:
    print(f"kept existing {auth_item}")

capabilities = ",".join([
    f"acquire:{model_item}#token",
    f"acquire:{auth_item}#agent_auth_secret",
])
answer = json.loads(invoke([
    "token-mint",
    consumer,
    "--capabilities",
    capabilities,
    "--workload-public-key-file",
    str(public_key),
    "--replace-capabilities",
]))
print(
    f"registered {consumer}: workload_bound={answer.get('workload_bound')} "
    f"capabilities=2 expires_at={answer.get('expires_at')}"
)
