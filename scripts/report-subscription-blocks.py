#!/usr/bin/env python3
"""Report or clear provider-authentication blocks in the subscription ledger.

A credential inside a recorded block is skipped before any provider call, so a
subscription that has just been repaired still looks dead until the block
expires. The dispatcher says only "all bounded credentials unavailable", and
the ledger is the only place that distinguishes the two.

With no arguments this prints non-secret block and usage metadata. Use
`--clear-auth SUBSCRIPTION_ID` after repairing one credential; the command
refuses to remove rate-limit or capacity blocks.
"""

from __future__ import annotations

import datetime
import json
import os
import pathlib
import stat
import sys
import tempfile

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


if len(sys.argv) > 1:
    if len(sys.argv) != 3 or sys.argv[1] != "--clear-auth":
        raise SystemExit(f"usage: {sys.argv[0]} [--clear-auth SUBSCRIPTION_ID]")
    subscription_id = sys.argv[2]
    entry = entries.get(subscription_id)
    block = entry.get("block") if isinstance(entry, dict) else None
    reason = block.get("reason") if isinstance(block, dict) else None
    if not isinstance(reason, str) or not any(
        marker in reason.lower()
        for marker in ("authentication", "invalid_grant", "oauth", "token is expired")
    ):
        raise SystemExit(f"{subscription_id}: no provider-authentication block to clear")
    entry["block"] = None
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        delete=False,
    ) as output:
        json.dump(ledger, output, indent=2, sort_keys=True)
        output.write("\n")
        temporary = pathlib.Path(output.name)
    temporary.chmod(stat.S_IMODE(path.stat().st_mode))
    os.replace(temporary, path)
    print(f"{subscription_id}: cleared provider-authentication block")
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
