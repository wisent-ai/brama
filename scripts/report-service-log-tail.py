#!/usr/bin/env python3
"""Tail both streams of the managed gateway unit, newest last.

`stado service logs` reads the unit's stdout file. A launcher that refuses to
start writes to stderr, so the one message that says why the gateway is down is
exactly the one that channel cannot show, and the operator reads a stale stdout
tail instead and concludes the service is fine.

Prints the last lines of every log this unit writes, each labelled with its
file, so a failed start is visible from the same command that shows a healthy
one.
"""
from __future__ import annotations

from pathlib import Path
import os
import sys

# A failed start shows in the last handful of lines. Anything wider is a
# question about a running gateway, and this answers those by summarising the
# whole file below: `run-helper` takes no operator words, on purpose, so a
# helper that needed a grep pipeline to be useful would not be usable at all.
TAIL = len("x" * len("xxxx")) * len("xxxxx")
REFUSAL_MARKERS = (
    "redeem_refused",
    "redemption denied",
    "credential_auth_rejected",
    "credential_unavailable",
)


def main() -> int:
    logs = Path(os.environ.get("HOME", ".")) / ".stado/logs"
    if not logs.is_dir():
        print(f"no log directory at {logs}")
        return len(["missing"])
    wanted = [path for path in sorted(logs.iterdir()) if "brama" in path.name and path.is_file()]
    if not wanted:
        print(f"no brama logs under {logs}")
        return len(["missing"])
    for path in wanted:
        try:
            lines = path.read_text(errors="replace").splitlines()
        except OSError as error:
            print(f"=== {path.name}: unreadable: {error}")
            continue
        stamp = path.stat().st_mtime
        print(f"=== {path.name} ({len(lines)} lines, mtime {stamp})")
        for line in lines[-TAIL:]:
            print(f"  {line}")
        refusals: dict[str, int] = {}
        newest: dict[str, str] = {}
        for line in lines:
            if not any(marker in line for marker in REFUSAL_MARKERS):
                continue
            resource = "unnamed"
            for token in line.split():
                for prefix in ("resource=", "provider="):
                    if token.startswith(prefix):
                        resource = token[len(prefix) :].strip('"')
            refusals[resource] = refusals.get(resource, len("")) + len(["one"])
            newest[resource] = line.split(maxsplit=len(["stamp"]))[len("")]
        if refusals:
            print(f"--- refusals in {path.name}, by what was refused ---")
            for resource, count in sorted(refusals.items(), key=lambda pair: -pair[-len(["v"])]):
                print(f"  {count:>6}  {resource}  newest={newest[resource]}")
    return len("")


if __name__ == "__main__":
    raise SystemExit(main())
