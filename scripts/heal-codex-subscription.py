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
import select
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


def blocked_codex(ledger: dict) -> dict[str, dict[str, object]]:
    """Codex subscriptions the provider refused, keyed by subscription.

    The value carries the moment the refusal was recorded, which is what makes
    one episode distinguishable from the next. A block sits in the ledger until
    it lapses, and the ledger is rewritten on every request, so without that
    marker a single standing refusal would buy a token exchange on every write.
    """
    entries = ledger.get("subscriptions") if isinstance(ledger, dict) else None
    if not isinstance(entries, dict):
        entries = ledger if isinstance(ledger, dict) else {}
    found: dict[str, dict[str, object]] = {}
    for name, entry in entries.items():
        if "codex" not in name or not isinstance(entry, dict):
            continue
        block = entry.get("block")
        if not isinstance(block, dict):
            continue
        reason = str(block.get("reason", ""))
        if any(mark in reason.lower() for mark in AUTH_MARKS):
            found[name] = {"reason": reason, "recorded_at": block.get("recorded_at_ms")}
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


def await_change(path: pathlib.Path) -> None:
    """Block until the ledger is written, without polling and without a clock.

    A watch-triggered launchd job is invisible to the fleet's health view: its
    normal state between wakeups is "not running", which every status command
    reports as a missing service. Staying resident and blocking here costs no
    cycles while everything works and leaves the unit legible as running.

    The file is rewritten rather than appended, so the descriptor is reopened
    on every wakeup; watching a deleted inode would go quiet for good.
    """
    queue = select.kqueue()
    descriptor = os.open(str(path), os.O_RDONLY)
    try:
        event = select.kevent(
            descriptor,
            filter=select.KQ_FILTER_VNODE,
            flags=select.KQ_EV_ADD | select.KQ_EV_ENABLE | select.KQ_EV_CLEAR,
            fflags=select.KQ_NOTE_WRITE
            | select.KQ_NOTE_EXTEND
            | select.KQ_NOTE_DELETE
            | select.KQ_NOTE_RENAME,
        )
        # Register expecting nothing back, then wait for exactly this one event.
        queue.control([event], len([]), None)
        queue.control(None, len([event]), None)
    finally:
        os.close(descriptor)
        queue.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    # Three ways to run, and the bare one is the deployed one: with no flags
    # this stays resident and blocks until the ledger changes, which is the
    # exact moment the dispatcher records a refusal. `--once` is for a probe,
    # `--interval` for a host where the change notification is unavailable.
    parser.add_argument("--interval", type=int, help="seconds between checks instead of watching")
    parser.add_argument("--once", action="store_true", help="check once and exit")
    options = parser.parse_args()

    if not CODEX.is_file():
        print(f"no codex CLI at {CODEX}; nothing to heal with", flush=True)
        return os.EX_UNAVAILABLE

    # Which refusal episode was already answered, per subscription. A standing
    # block would otherwise buy an exchange on every ledger write, and the
    # ledger is written on every request.
    answered: dict[str, object] = {}

    while True:
        try:
            ledger = json.loads(LEDGER.read_text())
        except FileNotFoundError:
            ledger = {}
        except json.JSONDecodeError as error:
            print(f"ledger unreadable: {error}", flush=True)
            ledger = {}

        fresh = {
            name: entry
            for name, entry in blocked_codex(ledger).items()
            if answered.get(name) != entry["recorded_at"]
        }
        if fresh:
            for name, entry in fresh.items():
                print(f"refused: {name}: {entry['reason']}", flush=True)
                answered[name] = entry["recorded_at"]
            if exchange():
                bank()
        if options.once:
            return os.EX_OK
        if options.interval is None:
            await_change(LEDGER)
        else:
            time.sleep(options.interval)


if __name__ == "__main__":
    sys.exit(main())
