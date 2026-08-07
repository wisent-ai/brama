#!/usr/bin/env python3
"""Send this host's agent signing secret to another host through Stado.

Two hosts holding different values under `agent:wisent-app` produce a refusal
that is correct on both sides: the caller signs with its copy, the gateway
verifies with its own, and the request is rejected as if the credential were
forged. The gateway is the verifier for the whole fleet, so its copy is the one
a caller has to sign with.

Runs on the gateway host and pushes the field through `stado host
install-credential`, which reads the selected store here and lands an
owner-only file there. The value never appears in a command line, an argument
list or this output.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
STADO = HOME / ".stado" / "bin" / "stado"
TARGET = os.environ.get("AGENT_SECRET_TARGET", "lukasz-macbook")
ITEM = os.environ.get("AGENT_SECRET_ITEM", "agent:wisent-app")
FIELD = os.environ.get("AGENT_SECRET_FIELD", "value")
NAME = os.environ.get("AGENT_SECRET_NAME", "jeden-agent-auth-secret")

if not STADO.exists():
    raise SystemExit(f"stado is unavailable here: {STADO}")

# The transfer reads the selected store as whichever consumer the environment
# presents, and the default one holds no grant here. Asking as that default and
# reading the 403 as "nobody may read this" is how a grant that exists gets
# reported as missing: `local-operator` carries `read agent:wisent-app#value`
# on this host and has a token file beside the others.
CONSUMER = os.environ.get("AGENT_SECRET_CONSUMER", "local-operator")
TOKEN_FILE = os.environ.get(
    "AGENT_SECRET_TOKEN_FILE", str(HOME / ".stado" / f"{CONSUMER}-skarbiec-token")
)
done = subprocess.run(
    [str(STADO), "host", "install-credential", TARGET, ITEM, FIELD, NAME],
    capture_output=True,
    text=True,
    env={
        **os.environ,
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "WC_SKARBIEC_CONSUMER": CONSUMER,
        "WC_SKARBIEC_TOKEN_FILE": TOKEN_FILE,
    },
)
print("target:", TARGET, "item:", ITEM, "field:", FIELD, "name:", NAME)
print("exit:", done.returncode)
for line in (done.stdout or "").splitlines():
    print("  out:", line)
for line in (done.stderr or "").splitlines():
    print("  err:", line)
