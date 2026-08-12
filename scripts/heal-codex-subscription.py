#!/usr/bin/env python3
"""Keep the banked Codex credential fresh, so a live subscription stays usable.

The gateway sends `tokens.access_token` verbatim and never refreshes it. The
token is short-lived, so a paid, logged-in subscription serves for a few hours
and then every call fails with "authentication token is expired" -- a sentence
that reads as a dead account. It is not one: `codex login status` still reports
the session, and the vendor CLI exchanges the refresh token unattended.

Nothing in the fleet performed that exchange, so the repair had to be done by
hand each time the token lapsed. This watches the dispatcher's own usage ledger
and, when it records an authentication block against a Codex subscription, runs
the exchange and re-banks the refreshed session where the gateway reads it.

Event-driven on purpose: the exchange costs a model call, so it happens when the
credential is actually refused rather than on a timer.

Prints what it observed and did. No token is printed.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

HOME = pathlib.Path.home()
LEDGER = HOME / ".config/brama/subscription-usage.json"
CODEX = pathlib.Path("/opt/homebrew/bin/codex")
BANKER = HOME / ".stado/bin/install-codex-subscription-credential"
PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
# The dispatcher writes the provider's own words; these are the ones that mean
# "this credential was refused for who it is", as opposed to a rate limit.
AUTH_MARKS = ("authentication", "expired", "unauthorized", "invalid_grant")
PROBE = "Reply with the single word PROBE."


def blocked_codex(ledger: dict) -> list[str]:
    entries = ledger.get("subscriptions") if isinstance(ledger, dict) else None
    if not isinstance(entries, dict):
        entries = ledger if isinstance(ledger, dict) else {}
    found = []
    for name, entry in entries.items():
        if "codex" not in name or not isinstance(entry, dict):
            continue
        block = entry.get("block")
        if not isinstance(block, dict):
            continue
        reason = str(block.get("reason", "")).lower()
        if any(mark in reason for mark in AUTH_MARKS):
            found.append(f"{name}: {block.get('reason')}")
    return found


def exchange() -> bool:
    """Force the CLI to swap the refresh token for a fresh access token."""
    environment = {**os.environ, "PATH": PATH}
    result = subprocess.run(
        [str(CODEX), "exec", "--skip-git-repo-check", PROBE],
        capture_output=True,
        text=True,
        env=environment,
        timeout=None,
    )
    if result.returncode:
        print(f"  exchange failed: {' '.join(result.stderr.split())[:200]}", flush=True)
        return False
    return True


def bank() -> bool:
    if not BANKER.is_file():
        print(f"  cannot bank: {BANKER} is not installed", flush=True)
        return False
    result = subprocess.run(
        [str(BANKER)], capture_output=True, text=True, env={**os.environ, "PATH": PATH}
    )
    print("  " + " ".join(result.stdout.split())[:200], flush=True)
    return result.returncode == os.EX_OK


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    # launchd can wake this on every write to the ledger, which is the exact
    # moment the dispatcher records a refusal, so the normal installation needs
    # no interval at all. The polling form stays for a host without WatchPaths.
    parser.add_argument("--interval", type=int, help="seconds between checks when polling")
    parser.add_argument("--once", action="store_true", help="check once and exit")
    options = parser.parse_args()
    if not options.once and options.interval is None:
        parser.error("either --once or --interval is required")

    if not CODEX.is_file():
        print(f"no codex CLI at {CODEX}; nothing to heal with", flush=True)
        return os.EX_UNAVAILABLE

    while True:
        try:
            ledger = json.loads(LEDGER.read_text())
        except FileNotFoundError:
            ledger = {}
        except json.JSONDecodeError as error:
            print(f"ledger unreadable: {error}", flush=True)
            ledger = {}

        refused = blocked_codex(ledger)
        if refused:
            for line in refused:
                print(f"refused: {line}", flush=True)
            if exchange():
                bank()
        if options.once:
            return os.EX_OK
        time.sleep(options.interval)


if __name__ == "__main__":
    sys.exit(main())
