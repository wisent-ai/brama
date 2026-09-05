#!/usr/bin/env python3
"""Say what shape a credential field has, without revealing it.

Whether `provider:X` may be routed to a given item is a question about what the
item holds: a bearer is a transcription, a configuration blob is a guess. The
difference is visible from the shape alone -- length, whether it parses as JSON,
and if so which keys it carries -- and none of that discloses the value.

Read-only. Prints lengths, types and key names, never a value or a fragment.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
ITEMS = tuple(
    os.environ.get("SHAPE_ITEMS", "codex-reauth-config,provider:openai,provider:local-openai").split(",")
)

for item in ITEMS:
    opened = subprocess.run(
        [str(SKARBIEC), "get", item],
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "SKARBIEC_VAULT_FILE": str(VAULT),
        },
    )
    print("item:", item)
    if opened.returncode:
        print("  unreadable:", " ".join((opened.stderr or "").split()))
        continue
    fields = (json.loads(opened.stdout).get("fields") or {})
    for name, value in sorted(fields.items()):
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
