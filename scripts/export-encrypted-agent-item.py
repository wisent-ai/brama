#!/usr/bin/env python3
"""Export one encrypted vault record for audited host-to-host recovery."""

import json
import os
from pathlib import Path

home = Path(os.environ.get("HOME", "."))
service_env = Path(os.environ.get("BRAMA_SERVICE_ENV_FILE", home / ".config/brama/service.env"))
vault_setting = os.environ.get("SKARBIEC_VAULT_FILE", "")
if not vault_setting and service_env.is_file():
    for line in service_env.read_text(errors="replace").splitlines():
        name, separator, value = line.partition("=")
        if separator and name.strip() == "SKARBIEC_VAULT_FILE":
            vault_setting = value.strip().strip('"').strip("'")
            break
vault = Path(vault_setting) if vault_setting else home / ".stado/skarbiec.vault.json"
item = os.environ.get("AGENT_SECRET_ITEM", "agent:wisent-app")
document = json.loads(vault.read_text(encoding="utf-8"))
record = (document.get("items") or {}).get(item)
if not isinstance(record, dict):
    raise SystemExit(f"encrypted item is absent: {item}")
print(json.dumps({"item": item, "record": record}, separators=(",", ":")))
