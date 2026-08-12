#!/usr/bin/env python3
"""Authorize Probierz on the Codex subscriptions declared for it.

Runs on the Skarbiec owner host. Existing payloads and recipients are preserved;
only the public brama:agent:probierz tag is added. Secret values stay on stdin to
Skarbiec and are never printed or placed in argv.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess

HOME = Path.home()
SKARBIEC = HOME / ".stado/bin/skarbiec"
VAULT = Path(os.environ.get("SKARBIEC_VAULT_FILE", HOME / ".stado/skarbiec.vault.json"))
ITEMS = (
    "provider:codex:brama-sub-wisent-app-codex-primary",
    "provider:codex:brama-sub-wisent-app-codex-secondary",
)
TAG = "brama:agent:probierz"
ENVIRONMENT = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": str(VAULT),
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}


def invoke(arguments: list[str], *, payload: str | None = None) -> str:
    result = subprocess.run(
        [str(SKARBIEC), *arguments],
        input=payload,
        capture_output=True,
        text=True,
        check=False,
        env=ENVIRONMENT,
    )
    if result.returncode:
        detail = (result.stderr or result.stdout).strip().replace("\n", " ")
        raise SystemExit(f"skarbiec {' '.join(arguments)} refused: {detail}")
    return result.stdout


listing = json.loads(invoke(["list"]))
metadata = {entry["id"]: entry for entry in listing if isinstance(entry, dict) and isinstance(entry.get("id"), str)}
for item in ITEMS:
    entry = metadata.get(item)
    if entry is None or entry.get("deleted", False):
        raise SystemExit(f"subscription item is missing: {item}")
    tags = sorted({*(str(tag) for tag in entry.get("tags") or []), TAG})
    if TAG in (entry.get("tags") or []):
        print(f"kept {item}: {TAG}")
        continue
    recipients = [str(recipient) for recipient in entry.get("recipients") or []]
    payload = invoke(["get", item])
    arguments = ["set-json", item, "--type", str(entry.get("kind") or "bundle"), "--tags", ",".join(tags)]
    if recipients:
        arguments.extend(["--recipients", ",".join(recipients)])
    invoke(arguments, payload=payload)
    print(f"added {TAG} to {item}; recipients={len(recipients)}")
