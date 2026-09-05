#!/usr/bin/env python3
"""Time how long each gateway item takes to decrypt, and whether it finishes.

A keyring whose key carries a passphrase turns every use into a pinentry
prompt. On a headless host nobody answers it, so the call does not fail -- it
waits. Upstream that reads as a request the gateway never answers, and the
client reports a timeout against a server that looks healthy to every request
that happens not to need that item.

Bounded and read-only: each attempt is given a few seconds and killed, so this
probe cannot itself hang. Prints item names, elapsed seconds and exit codes,
never a value.
"""
from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
KEYRINGS = (
    os.environ.get("BRAMA_GNUPG_HOME", str(HOME / ".gnupg")),
    str(HOME / ".stado" / "services" / "brama" / "gnupg"),
)
ITEMS = ("agent:wisent-app", "provider:local-openai", "provider:openai")
LIMIT_SECONDS = len("........")

for keyring in KEYRINGS:
    print("keyring:", keyring)
    for item in ITEMS:
        environment = {
            **os.environ,
            "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "SKARBIEC_VAULT_FILE": str(VAULT),
            "GNUPGHOME": keyring,
            # Refuse to open a prompt: without a terminal or an answer, the call
            # must fail fast rather than wait for a human who is not there.
            "GPG_TTY": "",
        }
        started = time.monotonic()
        try:
            done = subprocess.run(
                [str(SKARBIEC), "get", item],
                capture_output=True,
                text=True,
                env=environment,
                timeout=LIMIT_SECONDS,
            )
            elapsed = time.monotonic() - started
            print(f"  {item}: exit={done.returncode} elapsed={elapsed:.2f}s")
            if done.returncode:
                print("    detail:", " ".join((done.stderr or "").split()))
        except subprocess.TimeoutExpired:
            print(f"  {item}: TIMED OUT after {LIMIT_SECONDS}s -- waiting on something")
