#!/usr/bin/env python3
"""Report AI text product and Brama introspection grants without secret values."""

from __future__ import annotations

import json
import os
from pathlib import Path
import stat
import subprocess


HOME = Path.home()
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = HOME / ".stado" / "skarbiec.vault.json"
CONSUMERS = {
    "ai-text-detector-web",
    "ai-text-generator-web",
    "brama-token-introspector",
}

environment = {**os.environ, "SKARBIEC_VAULT_FILE": str(VAULT)}
result = subprocess.run(
    [str(SKARBIEC), "tokens"],
    capture_output=True,
    text=True,
    check=False,
    env=environment,
)
if result.returncode:
    detail = " ".join((result.stderr or result.stdout or "command failed").split())
    raise SystemExit(f"reading token grants failed: {detail}")

grants = json.loads(result.stdout)
report = []
for grant in grants:
    if not isinstance(grant, dict) or grant.get("consumer") not in CONSUMERS:
        continue
    capabilities = grant.get("capabilities") or []
    report.append(
        {
            "consumer": grant.get("consumer"),
            "expires_at": grant.get("expires_at"),
            "capabilities": [
                {
                    "action": capability.get("action"),
                    "item": capability.get("item"),
                    "field": capability.get("field"),
                }
                for capability in capabilities
                if isinstance(capability, dict)
            ],
        }
    )

introspection_path = HOME / ".stado" / "brama-token-introspector-skarbiec-token"
file_report = {"exists": introspection_path.is_file()}
if introspection_path.is_file():
    metadata = introspection_path.stat()
    file_report.update(
        {
            "bytes": metadata.st_size,
            "mode": stat.S_IMODE(metadata.st_mode),
        }
    )

print(json.dumps({"grants": report, "introspection_file": file_report}, sort_keys=True))
