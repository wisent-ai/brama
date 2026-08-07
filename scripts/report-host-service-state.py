#!/usr/bin/env python3
"""Report why the always-on Brama unit is not serving, without changing it.

`launchctl` reporting a unit as loaded says nothing about whether the process
inside it stayed up, and the health beacon only carries the verdict. When the
gateway is down, the exit status and the tail of its own stderr are the two
facts that separate "misconfigured" from "crashing" from "never started".

Read-only. Prints operational text only; the log is Brama's own diagnostics,
which never contain credential values.
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
UNIT = os.environ.get("BRAMA_UNIT", "com.wisent.always-on.brama")
LOG_LINES = int(os.environ.get("BRAMA_LOG_LINES", "25"))
LOGS = (
    HOME / ".stado" / "logs" / "brama-always-on.err",
    HOME / ".stado" / "logs" / "brama-always-on.out",
)
PLIST = Path("/Library/LaunchDaemons") / f"{UNIT}.plist"


def launchctl_state() -> str:
    done = subprocess.run(
        ["/bin/launchctl", "print", f"system/{UNIT}"],
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode:
        return f"launchctl print failed: {done.stderr.strip() or done.returncode}"
    wanted = ("state = ", "last exit code", "last exit reason", "program = ", "runs = ")
    picked = [
        line.strip()
        for line in done.stdout.splitlines()
        if any(marker in line for marker in wanted)
    ]
    return "\n  ".join(picked) if picked else "no state lines reported"


print("unit:", UNIT)
print("plist present:", PLIST.exists())
print("launchctl:")
print(" ", launchctl_state())
for log in LOGS:
    print()
    print("log:", log)
    if not log.exists():
        print("  absent")
        continue
    lines = log.read_text(errors="replace").splitlines()
    for line in lines[-LOG_LINES:]:
        print("  " + line)

# The launcher drives `skarbiec capability-issue`; a broker binary predating
# that verb takes the gateway down with "unknown command", which reads like a
# policy failure and is actually a version skew. Report both binaries' versions
# and whether the verb exists, so the two are never confused again.
for candidate in (
    HOME / ".stado" / "bin" / "skarbiec",
    Path("/usr/local/bin/skarbiec"),
    Path("/opt/homebrew/bin/skarbiec"),
):
    print()
    print("binary:", candidate)
    if not candidate.exists():
        print("  absent")
        continue
    version = subprocess.run(
        [str(candidate), "--version"], capture_output=True, text=True, check=False
    )
    listing = subprocess.run(
        [str(candidate), "help"], capture_output=True, text=True, check=False
    )
    print("  version:", (version.stdout or version.stderr).strip())
    print("  capability-issue:", "capability-issue" in listing.stdout)
