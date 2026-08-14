#!/usr/bin/env python3
"""Verify Brama's token-introspection authority without a provider request."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import urllib.error
import urllib.request


HOME = Path.home()
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = HOME / ".stado" / "skarbiec.vault.json"
AUTHORITY_TOKEN = HOME / ".stado" / "brama-token-introspector-skarbiec-token"
PROBE_CONSUMER = "brama-introspection-probe"
CAPABILITY = "call:brama#openai/gpt-5-mini"
ORIGIN = "http://127.0.0.1:8895"

environment = {**os.environ, "SKARBIEC_VAULT_FILE": str(VAULT)}


def invoke(*arguments: str) -> str:
    result = subprocess.run(
        [str(SKARBIEC), *arguments],
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )
    if result.returncode:
        detail = " ".join((result.stderr or result.stdout or "command failed").split())
        raise RuntimeError(f"skarbiec {' '.join(arguments)} refused: {detail}")
    return result.stdout


probe = json.loads(
    invoke(
        "token-mint",
        PROBE_CONSUMER,
        "--capabilities",
        CAPABILITY,
        "--ttl-seconds",
        "300",
        "--replace-capabilities",
    )
)
try:
    request = urllib.request.Request(
        f"{ORIGIN}/v1/tokens/introspect",
        data=json.dumps({"token": probe["token"]}, separators=(",", ":")).encode(),
        headers={
            "Authorization": f"Bearer {AUTHORITY_TOKEN.read_text().strip()}",
            "Content-Type": "application/json",
            "X-Consumer": "brama-token-introspector",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            body = json.load(response)
            status = response.status
    except urllib.error.HTTPError as error:
        status = error.code
        body = json.loads(error.read().decode())
finally:
    invoke("token-revoke", PROBE_CONSUMER)

print(
    json.dumps(
        {
            "status": status,
            "active": body.get("active"),
            "consumer": body.get("consumer"),
            "capabilities": body.get("capabilities"),
            "error": body.get("error"),
        },
        sort_keys=True,
    )
)
