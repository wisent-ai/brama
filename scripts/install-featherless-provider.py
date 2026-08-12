#!/usr/bin/env python3
"""Import the staged Featherless key into this host's owner-local Skarbiec vault."""

import json
import os
import pathlib
import subprocess

HOME = pathlib.Path.home()
SERVICE_ENV = HOME / ".config" / "brama" / "service.env"
IMPORT_FILE = HOME / ".stado" / "featherless-api-key.import"
ITEM = "provider:featherless"

settings: dict[str, str] = {}
for line in SERVICE_ENV.read_text(encoding="utf-8").splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

router = settings.get("ENTITLEMENTS_ROUTER_BIN")
vault = settings.get("SKARBIEC_VAULT_FILE")
if not router or not vault:
    raise SystemExit("service env must name ENTITLEMENTS_ROUTER_BIN and SKARBIEC_VAULT_FILE")

api_key = IMPORT_FILE.read_text(encoding="utf-8").strip()
if not 16 <= len(api_key) <= 512 or any(character.isspace() for character in api_key):
    raise SystemExit("staged Featherless key has an invalid shape")

payload = {
    "schema": "skarbiec.item.v2",
    "kind": "api-key",
    "fields": {"api_key": api_key},
    "context": {"provider": "featherless"},
}
environment = {**os.environ, **settings}
stored = subprocess.run(
    [router, "set-json", ITEM, "--type", "api-key"],
    input=json.dumps(payload),
    text=True,
    capture_output=True,
    env=environment,
    check=False,
)
if stored.returncode:
    raise SystemExit(f"cannot store {ITEM}: {stored.stderr.strip()}")

confirmed = subprocess.run(
    [router, "get", ITEM],
    text=True,
    capture_output=True,
    env=environment,
    check=False,
)
if confirmed.returncode:
    raise SystemExit(f"cannot confirm {ITEM}: {confirmed.stderr.strip()}")
document = json.loads(confirmed.stdout)
if document.get("fields", {}).get("api_key") != api_key:
    raise SystemExit(f"read-back mismatch for {ITEM}")

IMPORT_FILE.unlink()
print(f"installed {ITEM}; removed staged import file")
