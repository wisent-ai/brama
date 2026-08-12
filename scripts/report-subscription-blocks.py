#!/usr/bin/env python3
"""Report which subscription credentials the gateway is currently skipping.

A credential inside a recorded rate-limit block is skipped before any provider
call, so a subscription that has just been repaired still looks dead until the
block expires. The dispatcher says only "all bounded credentials unavailable",
which reads identically to a missing credential, and the ledger is the only
place that distinguishes the two.

Read-only. Prints subscription ids, block reasons and reset times; the ledger
holds no secret values.
"""

from __future__ import annotations

import datetime
import json
import os
import pathlib

LEDGER = (
    pathlib.Path(os.environ.get("XDG_STATE_HOME", pathlib.Path.home() / ".local/state"))
    / "brama"
    / "subscription-usage.json"
)
ALTERNATIVES = [
    LEDGER,
    pathlib.Path.home() / ".local/state/brama/subscription-usage.json",
    pathlib.Path.home() / ".config/brama/subscription-usage.json",
    pathlib.Path.home() / ".stado/brama/subscription-usage.json",
]


def when(value: object) -> str:
    if not isinstance(value, (int, float)) or value <= 0:
        return str(value)
    moment = datetime.datetime.fromtimestamp(value / 1000, datetime.timezone.utc)
    delta = moment - datetime.datetime.now(datetime.timezone.utc)
    minutes = int(delta.total_seconds() // 60)
    return f"{moment.isoformat(timespec='seconds')} ({minutes:+d} min)"


path = next((candidate for candidate in ALTERNATIVES if candidate.is_file()), None)
if path is None:
    print("no usage ledger found at any known location:")
    for candidate in ALTERNATIVES:
        print(f"  {candidate}")
    raise SystemExit(0)

print(f"ledger: {path}")
try:
    ledger = json.loads(path.read_text())
except json.JSONDecodeError as error:
    print(f"unreadable: {error}")
    raise SystemExit(1)

entries = ledger.get("subscriptions") if isinstance(ledger, dict) else None
if not isinstance(entries, dict):
    entries = ledger if isinstance(ledger, dict) else {}

# The gateway and this report disagreed once already, with the dispatcher
# skipping a credential this file called free. Guessing key names is what
# produced that, so the shape is printed rather than interpreted.
for name, entry in sorted(entries.items()):
    if not isinstance(entry, dict):
        print(f"{name}: {entry!r}")
        continue
    parts = []
    for key, value in sorted(entry.items()):
        if isinstance(value, dict):
            inner = ", ".join(
                f"{nested}={when(payload) if isinstance(payload, (int, float)) and payload > 1_000_000_000_000 else payload}"
                for nested, payload in sorted(value.items())
            )
            parts.append(f"{key}{{{inner}}}")
        elif isinstance(value, (int, float)) and value > 1_000_000_000_000:
            parts.append(f"{key}={when(value)}")
        elif isinstance(value, list):
            parts.append(f"{key}=<list:{len(value)}>")
        else:
            parts.append(f"{key}={value}")
    print(f"{name}: {'; '.join(parts)}")
