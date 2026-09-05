#!/usr/bin/env python3
"""Print the nonsecret Stado credential-store route used on this host."""

import json
import os
import subprocess
from pathlib import Path

home = Path(os.environ.get("HOME", "."))
paths = [home / ".config" / "stado" / "config.json", home / ".stado" / "config.json"]
for path in paths:
    if not path.is_file():
        continue
    document = json.loads(path.read_text(encoding="utf-8"))
    print("config:", path)
    print("credentials:", json.dumps(document.get("credentials"), sort_keys=True))
    print("secrets.skarbiec:", json.dumps((document.get("secrets") or {}).get("skarbiec"), sort_keys=True))
print("environment:")
for name in ("STADO_CREDENTIALS_STORE", "WC_SKARBIEC_URL", "WC_SKARBIEC_CONSUMER", "WC_SKARBIEC_TOKEN_FILE"):
    value = os.environ.get(name, "")
    print(f"  {name}={value}")

skarbiec = home / ".stado" / "bin" / "skarbiec"
listed = subprocess.run(
    [str(skarbiec), "tokens"],
    capture_output=True,
    text=True,
    env={
        **os.environ,
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "SKARBIEC_VAULT_FILE": str(home / ".stado" / "skarbiec.vault.json"),
    },
)
if listed.returncode:
    raise SystemExit("cannot list grants: " + " ".join(listed.stderr.split()))
for grant in json.loads(listed.stdout):
    if grant.get("consumer") == "stado-control-plane":
        print("stado-control-plane grant:", json.dumps(grant, sort_keys=True))
