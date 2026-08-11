#!/usr/bin/env python3
"""Install one encrypted gateway identity into this host's Skarbiec vault."""

import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

if len(sys.argv) != 2:
    raise SystemExit("usage: import-encrypted-agent-item.py ENCRYPTED_RECORD_JSON")
source = Path(sys.argv[1])
home = Path(os.environ.get("HOME", "."))
vault = Path(os.environ.get("SKARBIEC_VAULT_FILE", home / ".stado/skarbiec.vault.json"))
skarbiec = home / ".stado/bin/skarbiec"
payload = json.loads(source.read_text(encoding="utf-8"))
item = payload.get("item")
record = payload.get("record")
if item != "agent:wisent-app" or not isinstance(record, dict):
    raise SystemExit("encrypted record is not agent:wisent-app")
recipients = record.get("recipients") or []
kind = record.get("kind")
if kind != "stado-secret" or not all(isinstance(value, str) for value in recipients):
    raise SystemExit("encrypted identity metadata is incompatible")

temporary = vault.with_name(f".{vault.name}.{os.getpid()}.recovery")
shutil.copy2(vault, temporary)
os.chmod(temporary, 0o600)
try:
    document = json.loads(temporary.read_text(encoding="utf-8"))
    document.setdefault("items", {})[item] = record
    temporary.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    opened = subprocess.run(
        [str(skarbiec), "get", item],
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "SKARBIEC_VAULT_FILE": str(temporary),
        },
    )
    if opened.returncode:
        raise SystemExit("cannot decrypt recovered identity: " + " ".join(opened.stderr.split()))
    value = (json.loads(opened.stdout).get("fields") or {}).get("value")
    if not isinstance(value, str) or not value:
        raise SystemExit("recovered identity has no value field")
    canonical = json.dumps(
        {
            "schema": "skarbiec.item.v2",
            "kind": kind,
            "fields": {"value": value},
            "context": {},
        }
    )
    installed = subprocess.run(
        [str(skarbiec), "set-json", item, "--recipients", ",".join(recipients)],
        input=canonical,
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "SKARBIEC_VAULT_FILE": str(vault),
        },
    )
    if installed.returncode:
        raise SystemExit("cannot install recovered identity: " + " ".join(installed.stderr.split()))
    digest = hashlib.sha256(value.encode()).hexdigest()
    print("installed:", item, "digest:", digest[:16], "recipients:", len(recipients))
finally:
    temporary.unlink(missing_ok=True)
