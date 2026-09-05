#!/usr/bin/env python3
"""Report Cloudflare item fields and exact readers without printing values."""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

home = Path(os.environ.get("HOME", "."))
skarbiec = home / ".stado" / "bin" / "skarbiec"
vault = home / ".stado" / "skarbiec.vault.json"
environment = {**os.environ, "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin", "SKARBIEC_VAULT_FILE": str(vault)}

def describe(value: object) -> dict[str, object]:
    if isinstance(value, dict) and isinstance(value.get("value"), str):
        value = value["value"]
    if not isinstance(value, str):
        return {"kind": type(value).__name__}
    compact = value.replace("-", "")
    return {
        "kind": "string",
        "length": len(value),
        "looks_email": "@" in value,
        "looks_hex": bool(compact) and all(character in "0123456789abcdefABCDEF" for character in compact),
    }

items = ("platform-admin-cloudflare", "platform-admin-cloudflare-bobloo-tunnel")

def invoke(*arguments: str) -> object:
    result = subprocess.run([str(skarbiec), *arguments], capture_output=True, text=True, env=environment)
    if result.returncode:
        return {"error": " ".join((result.stderr or "command failed").split())}
    return json.loads(result.stdout)

listing = invoke("list")
if isinstance(listing, list):
    discovered = {
        entry.get("id")
        for entry in listing
        if isinstance(entry, dict)
        and isinstance(entry.get("id"), str)
        and "cloudflare" in entry["id"].lower()
    }
    items = tuple(sorted(set(items) | discovered))

tokens = invoke("tokens")
for item_id in items:
    item = invoke("get", item_id)
    raw_fields = item.get("fields") or {} if isinstance(item, dict) else {}
    fields = {name: describe(value) for name, value in raw_fields.items()}
    print(json.dumps({"item": item_id, "fields": fields}, sort_keys=True))
    holders = []
    for grant in tokens if isinstance(tokens, list) else []:
        for capability in grant.get("capabilities", []) or []:
            if capability.get("item") == item_id:
                holders.append({"consumer": grant.get("consumer"), "action": capability.get("action"), "field": capability.get("field")})
    print(json.dumps({"holders": holders}, sort_keys=True))
print(json.dumps({"token_files": sorted(path.name for path in (home / ".stado").glob("*-skarbiec-token"))}))
connector_files = []
for path in (home / ".stado").glob("*cloudflared*"):
    if path.is_file():
        metadata = path.stat()
        connector_files.append(
            {
                "name": path.name,
                "bytes": metadata.st_size,
                "mode": metadata.st_mode & 0o777,
            }
        )
print(json.dumps({"connector_files": connector_files}, sort_keys=True))
