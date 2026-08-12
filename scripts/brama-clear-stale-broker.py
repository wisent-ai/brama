#!/usr/bin/env python3
"""Clear a capability broker that outlived the launcher that started it.

The launcher starts its own broker and refuses to continue when the capability
socket is already owned by another process, because two brokers over one vault
is not something it can make safe. When a previous generation's broker survives
a restart - a stop that raced the launchd relaunch is enough - every subsequent
start prints `Skarbiec capability socket is owned by another process` and
exits, and launchd tries again forever.

Two things this file used to get wrong, both of which made it dangerous rather
than merely useless:

It read the unsuffixed `/tmp/brama-skarbiec`, while the launcher names the
runtime directory after the installation it is running. On a host that has run
more than one generation it therefore reported `no socket at ...` and left the
real socket in place, which is the same defect that made `brama-diagnose` read
capabilities issued by a bundle no longer on disk.

And it terminated every broker process it could see while trusting a docstring
to keep it away from a live one. "Run this only with the unit stopped" is not a
safeguard; on a serving host it killed the gateway's own broker. The unit being
loaded is not the discriminator either, because the relaunch loop this tool
exists to break is a loaded unit failing over and over. What settles it is
whether the gateway is actually serving: if it is, the broker holding the socket
belongs to it and is by definition not stale.
"""

import os
import pathlib
import signal
import subprocess
import time

BROKER_MARKER = "skarbiec-entitlements-router"
SERVE_MARKER = "capability-serve"
SERVER_MARKER = "bin/brama"
SELF_MARKER = "brama-clear-stale-broker"
SETTLE = float(len("wait a couple of seconds"))

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"
services = home / ".stado" / "services" / "brama"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

# Resolved exactly the way `start-with-skarbiec` resolves it, so this tool and the
# launcher cannot disagree about which directory is in play.
current = services / "current"
installation = current.resolve().name if current.exists() else ""
runtime_dir = pathlib.Path(
    settings.get("BRAMA_RUNTIME_DIR")
    or (f"/tmp/brama-skarbiec-{installation}" if installation else "/tmp/brama-skarbiec")
)
socket_path = runtime_dir / "socket" / "broker.sock"
print(f"installation: {installation or 'unresolved'}")
print(f"runtime dir: {runtime_dir}")


def processes():
    listing = subprocess.run(
        ["/bin/ps", "-Ao", "pid=,command="],
        capture_output=True,
        text=True,
        check=False,
    )
    # This script's own command line contains the product name, so without the
    # exclusion it reads itself as a serving gateway and refuses to run exactly
    # when it is needed: with the unit stopped, the only match left was this
    # process.
    mine = {os.getpid(), os.getppid()}
    for line in listing.stdout.splitlines():
        identifier, _, command = line.strip().partition(" ")
        if not identifier.isdigit():
            continue
        pid = int(identifier)
        if pid in mine or SELF_MARKER in command:
            continue
        yield pid, command.strip()


def brokers():
    return [
        (pid, command)
        for pid, command in processes()
        if BROKER_MARKER in command and SERVE_MARKER in command
    ]


def gateways():
    return [
        (pid, command)
        for pid, command in processes()
        if SERVER_MARKER in command and BROKER_MARKER not in command
    ]


serving = gateways()
if serving:
    for pid, command in serving:
        print(f"gateway alive: pid={pid} {command}")
    raise SystemExit(
        "refusing: the gateway is serving, so the broker holding the socket is its own. "
        "Stop the unit first - `stado service stop com.wisent.always-on.brama --host <host>` - "
        "and run this again; with the gateway down, a broker still on the socket is the stale one."
    )

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
