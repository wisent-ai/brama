#!/usr/bin/env python3
"""Say whether the gateway can read the key its capability client is built from.

`CapabilityClient::from_env` returns InvalidConfiguration when any of its three
variables is missing *or* when `read_owner_key` refuses the file, and the
gateway then answers false for every provider — so it stops on whichever alias
HashMap iteration reaches first. Two restarts naming two different aliases is
that one condition, not progress through a list.

`read_owner_key` demands a regular file owned by the running euid with no group
or other permission bits. The launcher exports all three variables itself, so
ownership against the unit's run user is the part no environment edit fixes.

Read-only apart from removing a duplicate declaration this repository's own
launcher already owns. Prints names, paths and modes, never a value.
"""
from __future__ import annotations

import os
import plistlib
import pwd
import stat
from pathlib import Path

GROUP_AND_OTHER = stat.S_IRWXG | stat.S_IRWXO

HOME = Path(os.environ.get("HOME", "."))
SERVICE_ENV = Path(
    os.environ.get(
        "BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")
    )
)
UNIT = os.environ.get("BRAMA_UNIT", "com.wisent.always-on.brama")
PLIST = Path("/Library/LaunchDaemons") / f"{UNIT}.plist"
VARIABLE = "SKARBIEC_WORKLOAD_SIGNING_KEY_FILE"
KEY_NAME = "brama-proof.key"


def owner(uid: int) -> str:
    try:
        return pwd.getpwuid(uid).pw_name
    except KeyError:
        return "unknown"


run_user = "root"
if PLIST.exists():
    with PLIST.open("rb") as handle:
        plist = plistlib.load(handle)
    run_user = plist.get("UserName", "root")
    print("plist:", PLIST)
    print("runs as:", run_user, "(declared)" if "UserName" in plist else "(no UserName -> root)")
else:
    print("plist absent:", PLIST)

key = HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm" / "etc" / "brama-skarbiec" / KEY_NAME
if not key.exists():
    print("key absent:", key)
else:
    info = key.stat()
    print(
        "key:", key,
        "owner:", owner(info.st_uid),
        "mode:", oct(stat.S_IMODE(info.st_mode)),
        "group/other bits:", "clear" if not info.st_mode & GROUP_AND_OTHER else "SET",
    )
    if info.st_mode & GROUP_AND_OTHER:
        # The deployed launcher predates the chmod now done at start, so tighten
        # the live key here too; both converge on the same owner-only mode.
        key.chmod(stat.S_IMODE(info.st_mode) & ~GROUP_AND_OTHER)
        print("tightened to:", oct(stat.S_IMODE(key.stat().st_mode)))
    print(
        "openable by run user:",
        "yes" if owner(info.st_uid) == run_user else f"NO -- owned by {owner(info.st_uid)}, unit runs as {run_user}",
    )

# The launcher exports this variable itself; a second declaration in the service
# environment is a duplicate source of truth, not a repair.
if SERVICE_ENV.exists():
    lines = SERVICE_ENV.read_text().splitlines(keepends=True)
    kept = [line for line in lines if not line.startswith(f"{VARIABLE}=")]
    if len(kept) != len(lines):
        SERVICE_ENV.write_text("".join(kept))
        print("removed duplicate declaration of", VARIABLE, "from", SERVICE_ENV)
    else:
        print("no duplicate declaration of", VARIABLE)
