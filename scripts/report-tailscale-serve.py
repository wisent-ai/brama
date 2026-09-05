#!/usr/bin/env python3
"""Report how this host exposes the gateway beyond its loopback listener.

Brama binds `127.0.0.1` on purpose -- the launcher unsets `BRAMA_BIND_ADDRESS`
and says so -- so everything reaching it from another machine arrives through
Tailscale. When a caller gets a connection refused on the tailnet address while
the process is healthy on loopback, the missing piece is that mapping, and
nothing in the service registry shows it.

Read-only: it runs the Tailscale status commands and prints what they say.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess

CANDIDATES = (
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
)


def binary() -> str | None:
    for path in CANDIDATES:
        if os.path.exists(path):
            return path
    return shutil.which("tailscale")


def run(argv: list[str]) -> str:
    try:
        done = subprocess.run(argv, capture_output=True, text=True)
    except (OSError, subprocess.SubprocessError) as error:
        return f"({type(error).__name__}: {error})"
    return (done.stdout or done.stderr).strip() or "(no output)"


tailscale = binary()
if tailscale is None:
    raise SystemExit("tailscale binary not found in the usual locations")

print("binary:", tailscale)
print("== serve status ==")
print(run([tailscale, "serve", "status"]))
print("== funnel status ==")
print(run([tailscale, "funnel", "status"]))

raw = run([tailscale, "status", "--json"])
try:
    state = json.loads(raw)
except ValueError:
    print("== status ==")
    print(raw)
else:
    node = state.get("Self", {})
    print("== node ==")
    print("  name:", node.get("DNSName", "?").rstrip("."))
    print("  addresses:", ", ".join(node.get("TailscaleIPs", []) or []))
    print("  backend:", state.get("BackendState", "?"))
