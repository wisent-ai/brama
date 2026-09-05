#!/usr/bin/env python3
"""Report provider-subscription field names and value shapes, never values."""

import json
import os
import subprocess
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
skarbiec = home / ".stado/bin/skarbiec"
document = json.loads(vault.read_text(encoding="utf-8"))
items = sorted(
    item
    for item in (document.get("items") or {})
    if item.startswith("provider:codex:brama-sub-wisent-app-codex-")
)


def describe(prefix: str, value: object) -> None:
    if isinstance(value, dict):
        print(f"    {prefix}: object keys={','.join(sorted(value))}")
        for key, nested in sorted(value.items()):
            describe(f"{prefix}.{key}", nested)
    elif isinstance(value, list):
        kinds = sorted({type(nested).__name__ for nested in value})
        print(f"    {prefix}: array length={len(value)} types={','.join(kinds)}")
    elif isinstance(value, str):
        print(f"    {prefix}: string length={len(value)}")
        try:
            nested = json.loads(value)
        except ValueError:
            nested = None
        if isinstance(nested, (dict, list)):
            describe(f"{prefix}.json", nested)
    else:
        print(f"    {prefix}: {type(value).__name__}")

for item in items:
    opened = subprocess.run(
        [str(skarbiec), "get", item],
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "SKARBIEC_VAULT_FILE": str(vault),
        },
    )
    print("item:", item)
    if opened.returncode:
        print("  unreadable:", " ".join(opened.stderr.split()))
        continue
    payload = json.loads(opened.stdout)
    print("  kind:", payload.get("kind"))
    for name, value in sorted((payload.get("fields") or {}).items()):
        text = value if isinstance(value, str) else json.dumps(value)
        shape = "opaque string"
        try:
            parsed = json.loads(text)
        except ValueError:
            parsed = None
        if isinstance(parsed, dict):
            shape = "json object, keys=" + ",".join(sorted(parsed))
        elif isinstance(parsed, list):
            shape = "json array"
        print(f"  {name}: length={len(text)} shape={shape}")
        if isinstance(parsed, (dict, list)):
            describe(name, parsed)
