#!/usr/bin/env python3
"""Materialize the four Brama Desktop subscription items without printing secrets."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = Path(os.environ.get("SKARBIEC_BIN", HOME / ".stado" / "bin" / "skarbiec"))
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", HOME / ".stado" / "skarbiec.vault.json")
)
RECIPIENTS = (
    "skarbiec-owner-charless-mini-20260804",
    "skarbiec-owner-20260728 <lukaszbartoszcze@wisent.ai>",
    "brama-rtx@wisent.local",
)
PRIMARY = "provider:codex:brama-sub-wisent-app-codex-primary"
SOURCES = {
    "provider:codex:brama-sub-wisent-app-codex-secondary": "codex-reauth-config",
    "provider:claude-code:brama-sub-wisent-app-claude-primary": "claude-reauth-config",
    "provider:kimi:brama-sub-wisent-app-kimi-primary": "kimi-reauth-config",
}
ENVIRONMENT = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "SKARBIEC_VAULT_FILE": str(VAULT),
}


def read_item(identifier: str) -> dict[str, object]:
    opened = subprocess.run(
        [str(SKARBIEC), "get", identifier],
        capture_output=True,
        text=True,
        env=ENVIRONMENT,
        check=False,
    )
    if opened.returncode:
        raise SystemExit(f"cannot read required source item {identifier}: {opened.stderr.strip()}")
    document = json.loads(opened.stdout)
    fields = document.get("fields")
    if not isinstance(fields, dict) or set(fields) != {"value"} or not fields["value"]:
        raise SystemExit(f"required source item {identifier} has an incompatible field shape")
    return document


def write_subscription(target: str, source: str) -> None:
    document = read_item(source)
    document["kind"] = "bundle"
    context = document.get("context")
    if not isinstance(context, dict):
        context = {}
        document["context"] = context
    context["source_item"] = source
    context["subscription_owner"] = "wisent-app"
    written = subprocess.run(
        [
            str(SKARBIEC),
            "set-json",
            target,
            "--recipients",
            ",".join(RECIPIENTS),
        ],
        input=json.dumps(document),
        capture_output=True,
        text=True,
        env=ENVIRONMENT,
        check=False,
    )
    if written.returncode:
        raise SystemExit(f"cannot write subscription item {target}: {written.stderr.strip()}")
    print(target)


read_item(PRIMARY)
print(PRIMARY)
for target, source in SOURCES.items():
    write_subscription(target, source)
