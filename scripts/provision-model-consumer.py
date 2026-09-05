#!/usr/bin/env python3
"""Provision one Brama caller without exposing either long-lived credential.

Run on the Skarbiec owner host. The request-signing secret is generated there,
written to the owner vault over stdin, and never printed or placed in argv. The
caller's workload public key authorizes one-time acquisition of exactly the
model-router bearer and its own signing secret.
"""

import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile


home = Path.home()
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("consumer", nargs="?", default="lem")
parser.add_argument("public_key", nargs="?")
parser.add_argument("--model", action="append", default=[],
                    help="Register the existing bearer for this exact Brama route; repeat for several routes.")
options = parser.parse_args()
consumer = options.consumer
public_key = (
    Path(options.public_key).expanduser().resolve()
    if options.public_key else
    home / ".stado" / "files" / f"{consumer}-workload-ed25519.pub.pem"
    if not options.model else None
)

if not consumer or any(character not in "abcdefghijklmnopqrstuvwxyz0123456789-_" for character in consumer):
    raise SystemExit("consumer must be one lowercase identifier")
if public_key is not None and not public_key.is_file():
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

if public_key is not None:
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

if options.model:
    item = json.loads(invoke(["get", model_item]))
    token = item.get("fields", {}).get("token")
    if not isinstance(token, str) or not token or token.strip() != token:
        raise SystemExit(f"{model_item} has no nonempty token field")
    grants = json.loads(invoke(["tokens"]))
    existing = next((entry for entry in grants if entry.get("consumer") == model_item), None)
    capabilities = set(f"call:brama#{model}" for model in options.model)
    if existing:
        if existing.get("workload_bound") or existing.get("audience") != consumer:
            raise SystemExit(f"{model_item} already belongs to a different grant; it was not changed")
        for capability in existing.get("capabilities", []):
            if capability.get("action") != "call" or capability.get("item") != "brama" or not capability.get("field"):
                raise SystemExit(f"{model_item} has a non-model grant; it was not changed")
            capabilities.add(f"call:brama#{capability['field']}")
    work = home / ".stado" / "work" / "brama-model-consumers"
    work.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f"{consumer}-", dir=work) as directory:
        token_path = Path(directory) / "bearer"
        descriptor = os.open(token_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as output:
            output.write(token)
        answer = json.loads(invoke([
            "token-mint", model_item,
            "--capabilities", ",".join(sorted(capabilities)),
            "--audience", consumer,
            "--token-file", str(token_path),
            "--replace-capabilities",
        ]))
    print(
        f"registered existing {model_item} bearer: models={len(capabilities)} "
        f"expires_at={answer.get('expires_at')}; stored token unchanged"
    )
