#!/usr/bin/env python3
"""Inventory Codex-related vault coordinates without opening their values."""

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
document = json.loads(vault.read_text(encoding="utf-8"))
for item, record in sorted((document.get("items") or {}).items()):
    tags = record.get("tags") or []
    haystack = " ".join([item, *tags]).lower()
    if "codex" not in haystack and "subscription" not in haystack:
        continue
    print(
        json.dumps(
            {
                "item": item,
                "kind": record.get("kind"),
                "state": record.get("state"),
                "tags": tags,
                "recipients": record.get("recipients") or [],
            },
            sort_keys=True,
        )
    )
