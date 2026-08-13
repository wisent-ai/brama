#!/usr/bin/env python3
"""Print the gateway's credential decisions, wherever they are in the log.

The dispatcher's answer to a caller is one sentence -- "all bounded credentials
unavailable" -- while the reason it reached that answer is a per-credential
warning written when the decision was made. Tailing the log misses those the
moment anything else is chatty, which is exactly when a request is being
diagnosed.

Read-only. Prints matching log lines; the gateway keeps credential values out
of them.
"""

from __future__ import annotations

import os
import pathlib
import re

HOME = pathlib.Path(os.environ.get("HOME", "."))
LOGS = HOME / ".stado" / "logs"
NAMES = ("brama-always-on.err", "brama-always-on.out", "com.wisent.always-on.brama.err")
PATTERN = re.compile(
    r"credential_blocked|credential_unavailable|credential_invalid_encoding"
    r"|credential_read_unrouted|capability_issue_refused|redemption denied"
    r"|carries no such field|no active .* credential"
)
KEEP = len("xx")
KINDS = (
    "credential_blocked",
    "credential_unavailable",
    "credential_invalid_encoding",
    "credential_read_unrouted",
    "capability_issue_refused",
    "redemption denied",
    "carries no such field",
    # The gateway refreshes an OAuth grant itself when a provider refuses one.
    # Whether that path ran, and what it said when it failed, is the difference
    # between a credential nobody can renew and one whose renewal is broken.
    "oauth_refresh_failed",
    "oauth_refresh_persist_failed",
    "credential_refreshed",
    "credential_retired",
)

for name in NAMES:
    path = LOGS / name
    if not path.is_file():
        continue
    print(f"== {path.name}")
    lines = path.read_text(errors="replace").splitlines()
    for kind in KINDS:
        matches = [line for line in lines if kind in line]
        if not matches:
            continue
        print(f"  {kind}: {len(matches)} line(s)")
        for line in matches[-KEEP:]:
            print("    ", line[:300])
