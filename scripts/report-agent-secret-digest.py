#!/usr/bin/env python3
"""Print a digest of one agent's request-signing secret, never the secret.

A gateway verifies an agent's signature with its own copy of that agent's
secret. When two hosts hold different values under the same item name, the
caller signs correctly and the gateway rejects it correctly, and the only
symptom is 401 on a request whose bearer is demonstrably accepted elsewhere.

Comparing digests across hosts settles that without moving the value.
"""
from __future__ import annotations

import hashlib
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
ITEM = os.environ.get("AGENT_SECRET_ITEM", "agent:wisent-app")
FIELD = os.environ.get("AGENT_SECRET_FIELD", "value")
CONSUMER = os.environ.get("AGENT_SECRET_CONSUMER", "local-operator")
TOKEN_FILE = os.environ.get(
    "AGENT_SECRET_TOKEN_FILE", str(HOME / ".stado" / "local-operator-skarbiec-token")
)
STADO = os.environ.get("STADO_BIN", str(HOME / ".stado" / "bin" / "stado"))
if not Path(STADO).exists():
    STADO = str(HOME / ".local" / "bin" / "stado")

environment = dict(os.environ)
environment["WC_SKARBIEC_CONSUMER"] = CONSUMER
environment["WC_SKARBIEC_TOKEN_FILE"] = TOKEN_FILE

done = subprocess.run(
    [STADO, "credentials", "get", "--field", FIELD, ITEM],
    capture_output=True,
    text=True,
    env=environment,
)
value = done.stdout.strip()
print("item:", ITEM, "field:", FIELD, "consumer:", CONSUMER)
if not value:
    print("unreadable:", " ".join((done.stderr or "").split()))
else:
    digest = hashlib.sha256(value.encode()).hexdigest()
    print("length:", len(value), "digest:", digest[: len("0123456789abcdef")])
