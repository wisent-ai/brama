#!/usr/bin/env python3
"""Verify that the managed Brama accepts a newly issued client bearer."""

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
PROBE_CONSUMER = "brama-dynamic-auth-probe"
CAPABILITY = "call:brama#openai/gpt-5-mini"
ORIGIN = "http://127.0.0.1:8080"

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
        f"{ORIGIN}/v1/models",
        headers={
            "Authorization": f"Bearer {probe['token']}",
            "Accept": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
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
            "model_count": len(body.get("data", [])),
            "error": body.get("error"),
        },
        sort_keys=True,
    )
)
