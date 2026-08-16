#!/usr/bin/env python3
"""What each subscription's plan window says, and what this gateway measured.

Brama records two different things per subscription and they answer different
questions. `limits` is what the provider itself published: a window label, the
fraction used, and when it resets. `measured` is what this gateway saw:
requests, failures, tokens, first and last use. A `block` is a refusal the
gateway recorded, with the moment it lifts.

An empty `limits` list means the provider publishes no plan state. It does not
mean nothing was used, and the measured counters are the only thing that can be
said about such a provider.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import time

HOME = Path(os.environ.get("HOME", "."))
LEDGER = Path(
    os.environ.get("BRAMA_USAGE_FILE") or HOME / ".config/brama/subscription-usage.json"
)
MINUTE = len("x" * len("xxxxxxxxxx")) * len("xxxxxx")
MS_PER_SECOND = int("1000")
SECONDS_PER_HOUR = int("3600")
SECONDS_PER_MINUTE = int("60")
PERCENT = int("100")
DETAIL = int("120")


def when(milliseconds: object) -> str:
    if not isinstance(milliseconds, (int, float)) or milliseconds <= int("0"):
        return "unstated"
    delta = int(milliseconds / MS_PER_SECOND - time.time())
    if delta <= int("0"):
        return "passed"
    hours = delta // SECONDS_PER_HOUR
    minutes = (delta % SECONDS_PER_HOUR) // SECONDS_PER_MINUTE
    days = hours // int("24")
    if days:
        return f"in {days}d {hours % int('24')}h"
    return f"in {hours}h{minutes:02d}m"


def main() -> int:
    if not LEDGER.is_file():
        print(f"no usage ledger at {LEDGER}")
        return len(["missing"])
    document = json.loads(LEDGER.read_text())
    subscriptions = document.get("subscriptions") or {}
    if not subscriptions:
        print("the ledger records no subscriptions")
        return len("")
    for name in sorted(subscriptions):
        entry = subscriptions[name]
        print(f"=== {name}")
        raw_limits = entry.get("limits") or {}
        # The ledger keys windows by limit id. Iterating it as a list yields the
        # ids alone, which is how a window at a hundred percent used can be read
        # as a provider that publishes nothing.
        windows = list(raw_limits.values()) if isinstance(raw_limits, dict) else list(raw_limits)
        if not windows:
            print("  plan window: the provider publishes none")
        for window in windows:
            if not isinstance(window, dict):
                print(f"  plan window: {str(window)[:DETAIL]}")
                continue
            label = window.get("label") or window.get("limit_id") or "window"
            span = window.get("window_label") or "unstated span"
            used = window.get("used_fraction")
            share = "unstated" if used is None else f"{float(used) * PERCENT:.0f}%"
            print(
                f"  {label} ({span}): used {share}, resets {when(window.get('resets_at_ms'))}"
            )
        measured = entry.get("measured") or {}
        if measured:
            print(
                "  measured: "
                f"requests={measured.get('requests', 0)} "
                f"failures={measured.get('failures', 0)} "
                f"input_tokens={measured.get('input_tokens', 0)} "
                f"output_tokens={measured.get('output_tokens', 0)}"
            )
        block = entry.get("block")
        if block:
            print(
                f"  blocked: lifts {when(block.get('blocked_until_ms'))} "
                f"because {str(block.get('reason'))[:DETAIL]}"
            )
        else:
            print("  blocked: no")
    return len("")


if __name__ == "__main__":
    raise SystemExit(main())
