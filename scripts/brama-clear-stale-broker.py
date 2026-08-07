#!/usr/bin/env python3
"""Clear a capability broker that outlived the launcher that started it.

The launcher starts its own broker and refuses to continue when the capability
socket is already owned by another process, because two brokers over one vault
is not something it can make safe. When a previous generation's broker survives
a restart - a stop that raced the launchd relaunch is enough - every subsequent
start prints `Skarbiec capability socket is owned by another process` and
exits, and launchd tries again forever.

Run this only with the unit stopped: with the unit stopped, any broker still
holding the socket is by definition the stale one.
"""

import os
import pathlib
import signal
import subprocess
import time

BROKER_MARKER = "skarbiec-entitlements-router"
SERVE_MARKER = "capability-serve"
SETTLE = float(len("wait a couple of seconds"))

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

runtime_dir = pathlib.Path(settings.get("BRAMA_RUNTIME_DIR") or "/tmp/brama-skarbiec")
socket_path = runtime_dir / "socket" / "broker.sock"


def brokers():
    listing = subprocess.run(
        ["/bin/ps", "-Ao", "pid=,command="],
        capture_output=True,
        text=True,
        check=False,
    )
    found = []
    for line in listing.stdout.splitlines():
        identifier, _, command = line.strip().partition(" ")
        if BROKER_MARKER in command and SERVE_MARKER in command:
            found.append((int(identifier), command.strip()))
    return found


live = brokers()
if not live:
    print("no broker process is holding the socket")
for pid, command in live:
    print(f"terminating pid={pid} {command}")
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        print(f"  pid={pid} was already gone")

if live:
    time.sleep(SETTLE)
    for pid, _ in brokers():
        print(f"pid={pid} ignored SIGTERM; sending SIGKILL")
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

if socket_path.exists() or socket_path.is_symlink():
    socket_path.unlink()
    print(f"removed {socket_path}")
else:
    print(f"no socket at {socket_path}")

print(f"remaining brokers: {len(brokers())}")
