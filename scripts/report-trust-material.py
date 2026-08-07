#!/usr/bin/env python3
"""Compare what the trust material pins against what is actually installed.

The broker refuses a redemption when the workload asking is not the workload
the registry describes, and the gateway keeps answering /health and listing
models while it happens. The failure surfaces only as a credential that is
"unavailable", with nothing saying which side disagreed.

This prints both sides: what the material names, and the same facts about the
binary that is really there.

Read-only. Prints paths and digests, never a key or a credential.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
BUNDLE = Path(os.environ.get("BRAMA_BUNDLE", str(HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm")))
CONFIG = Path(os.environ.get("BRAMA_SKARBIEC_CONFIG_DIR", str(BUNDLE / "etc" / "brama-skarbiec")))
REQUIRED = (
    "trust.json",
    "policy.json",
    "policy.sig",
    "registry.json",
    "registry.sig",
    "brama-proof.key",
    "worm-receipt",
)


def digest(path: Path) -> str:
    reader = hashlib.sha256()
    reader.update(path.read_bytes())
    return reader.hexdigest()


print("bundle:", BUNDLE, "exists:", BUNDLE.exists())
print("config:", CONFIG, "exists:", CONFIG.exists())
print("resolved:", BUNDLE.resolve() if BUNDLE.exists() else "-")
print("present:", [name for name in REQUIRED if (CONFIG / name).exists()])
print("missing:", [name for name in REQUIRED if not (CONFIG / name).exists()] or "none")

binary = BUNDLE / "bin" / "brama"
print("binary:", binary, "exists:", binary.exists())
if binary.exists():
    print("binary sha256:", digest(binary))
    print("binary uid/gid:", binary.stat().st_uid, binary.stat().st_gid)

registry = CONFIG / "registry.json"
if registry.exists():
    try:
        parsed = json.loads(registry.read_text(errors="replace"))
    except ValueError as error:
        print("registry.json does not parse:", error)
    else:
        for line in json.dumps(parsed, indent=len("xx"), sort_keys=True).splitlines():
            print("  ", line)
