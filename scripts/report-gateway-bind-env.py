#!/usr/bin/env python3
"""Report whether the running gateway actually received its bind address.

The launcher prints that it is serving the fleet address, the gateway binds
loopback anyway, and both statements are true at once if the variable never
reaches the process that reads it. The log cannot settle that; the process
environment can.

Prints only the two variables that decide binding, and only whether each is
present and what address it names -- an address is not a secret. Nothing else
from the environment is read out.
"""
from __future__ import annotations

import subprocess

WANTED = ("BRAMA_BIND_ADDRESS", "BRAMA_ENCRYPTED_PEER_IPS")

listing = subprocess.run(
    ["/bin/ps", "-Ao", "pid=,command="],
    capture_output=True,
    text=True,
)

pids = []
for line in listing.stdout.splitlines():
    stripped = line.strip()
    if "/bin/brama" in stripped and " serve" in stripped:
        pids.append(stripped.split(maxsplit=len("x"))[0])

print("running gateway pids:", pids or "none")

for pid in pids:
    environ = subprocess.run(
        ["/bin/ps", "eww", "-o", "command=", "-p", pid],
        capture_output=True,
        text=True,
    ).stdout
    print("== pid", pid)
    for name in WANTED:
        marker = name + "="
        if marker in environ:
            value = environ.split(marker, maxsplit=len("x"))[-1].split(maxsplit=len("x"))[0]
            print("  ", name, "=", value)
        else:
            print("  ", name, "ABSENT")
