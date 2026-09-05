#!/usr/bin/env python3
"""Mint the two server-side bearers used by the AI text products.

Run only on the Skarbiec owner host. The JSON response contains the new bearer
values so the operator can transfer them directly into the deployment secret
store; the script never places a bearer in argv or the environment.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess


HOME = Path.home()
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = HOME / ".stado" / "skarbiec.vault.json"
MODEL = "openai/gpt-5-mini"
TTL_SECONDS = str(365 * 24 * 60 * 60)
CONSUMERS = ("ai-text-detector-web", "ai-text-generator-web")

settings: dict[str, str] = {}
service_env = HOME / ".config" / "brama" / "service.env"
for line in service_env.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

environment = {**os.environ, **settings}
environment["SKARBIEC_VAULT_FILE"] = str(VAULT)

def mint(consumer: str, capability: str) -> dict[str, object]:
    result = subprocess.run(
        [
            str(SKARBIEC),
            "token-mint",
            consumer,
            "--capabilities",
            capability,
            "--ttl-seconds",
            TTL_SECONDS,
            "--replace-capabilities",
        ],
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )
    if result.returncode:
        detail = " ".join((result.stderr or result.stdout or "command failed").split())
        raise SystemExit(f"token-mint refused {consumer}: {detail}")
    payload = json.loads(result.stdout)
    token = payload.get("token")
    if not isinstance(token, str) or len(token) < 32:
        raise SystemExit(f"token-mint returned no bearer for {consumer}")
    return payload


introspection = mint("brama-token-introspector", "introspect:tokens")
introspection_path = HOME / ".stado" / "brama-token-introspector-skarbiec-token"
temporary_path = introspection_path.with_name(
    f".{introspection_path.name}.{os.getpid()}.tmp"
)
descriptor = os.open(temporary_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(descriptor, "w") as output:
    output.write(str(introspection["token"]))
    output.write("\n")
os.replace(temporary_path, introspection_path)

issued: dict[str, dict[str, object]] = {}
for consumer in CONSUMERS:
    payload = mint(consumer, f"call:brama#{MODEL}")
    issued[consumer] = {
        "token": payload["token"],
        "expires_at": payload.get("expires_at"),
        "workload_bound": payload.get("workload_bound"),
    }

print(json.dumps(issued, separators=(",", ":")))
