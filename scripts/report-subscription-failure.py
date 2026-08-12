#!/usr/bin/env python3
"""Print, in full, the lines explaining why a subscription credential failed.

The general state reporter truncates, and the interesting part of these lines
is at the end: the event name, the provider, and the sentence the gateway wrote
about what it could not do. This prints the whole line for the events that
decide a subscription dispatch.

Read-only.
"""
from __future__ import annotations

import os
import pathlib

HOME = pathlib.Path(os.environ.get("HOME", "."))
LOGS = HOME / ".stado" / "logs"
MARKERS = (
    "bounded credential",
    "credential_unavailable",
    "subscription_credential_redeem_failed",
    "subscription_credential_issue_failed",
    "provider_credential_read_refused",
    "capability_issue_failed",
    "redemption denied",
)
TAIL = len("aaaaaaaaaaaaaaaaaaaa")

candidates = sorted(LOGS.glob("*brama*"), key=lambda path: path.stat().st_mtime)
if not candidates:
    raise SystemExit(f"no gateway log under {LOGS}")

log = candidates[-len(["newest"])]
print("log:", log.name)
lines = log.read_text(errors="replace").splitlines()
hits = [line for line in lines if any(marker in line for marker in MARKERS)]
for line in hits[-TAIL:]:
    print(line.strip())
if not hits:
    print("no subscription failure lines in this log")
