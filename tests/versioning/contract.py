#!/usr/bin/env python3
"""Compare the version Cargo.toml declares with the one the AutoVersion rule
requires, allowing for release coordinates a failed build has consumed.

The rule itself is not here: `autoversion decide` answers what kind of change
the candidate surface is against the released one and which version that
requires. This script supplies what only this repository knows -- its
manifest, its baseline, its candidate surface and its commit history -- and
refuses when they disagree.

The comparison the shared workflow used to make demanded that the declared
version equal the required one exactly. The Stado release plane binds one
coordinate to one source revision before anything is published into it, so a
build that fails at that coordinate leaves it consumed: 0.4.2 and 0.4.3 were
claimed by builds that failed on charless-mac-mini, the next patch had to be
0.4.4, and the gate refused every commit on main from then on because the rule
still said 0.4.2. So a declared version may run ahead of the required one in
the same series, provided every coordinate it skips was itself declared by an
earlier commit on this branch -- the manifest history is the record of what
was submitted. A jump nothing accounts for is still refused, naming the first
coordinate that was never declared.

Usage: python3 tests/versioning/contract.py --autoversion <path> \
           [--candidate <surface.json>] [--self-check]
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
import tempfile

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
MANIFEST = REPOSITORY / "Cargo.toml"
BASELINE = REPOSITORY / "released-surface.json"
BREAKAGE = REPOSITORY / "declared-breakage.json"
SURFACE = pathlib.Path(__file__).resolve().parent / "surface.py"
CARGO_VERSION = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)
INTERNAL, ADDITIVE, BREAKING = "internal", "additive", "breaking"


class Refusal(Exception):
    """The gate's answer when the versions disagree; printed once, by main."""


def fail(message: str) -> None:
    raise Refusal(message)


def parse(version: str) -> tuple[int, int, int]:
    parts = version.split(".")
    if len(parts) != 3 or not all(part.isdigit() for part in parts):
        fail(f"{version} is not MAJOR.MINOR.PATCH")
    return tuple(int(part) for part in parts)


def declared_version() -> str:
    match = CARGO_VERSION.search(MANIFEST.read_text(encoding="utf-8"))
    if not match or not match.group(1):
        fail(f"{MANIFEST} has no non-empty [package].version")
    return match.group(1)

def released_version() -> str:
    baseline = json.loads(BASELINE.read_text(encoding="utf-8"))
    version = baseline.get("version")
    if not isinstance(version, str) or not version:
        fail(f"{BASELINE} names no released version")
    if not isinstance(baseline.get("surface"), list) or not baseline["surface"]:
        fail(f"{BASELINE} carries no surface")
    return version


def decide(autoversion: str, current: str, candidate: pathlib.Path, breaking: bool = False) -> dict:
    command = [autoversion, "decide", "--current", current, "--published-surface", str(BASELINE),
               "--candidate-surface", str(candidate), "--json"]
    if breaking:
        command.append("--breaking")
    completed = subprocess.run(command, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        fail(f"autoversion decide refused: {completed.stderr.strip() or completed.stdout.strip()}")
    return json.loads(completed.stdout)


def declared_breakage(declared: str) -> bool:
    """Whether the owner declared breakage the surface cannot show for this
    very version. It applies only while Cargo.toml still declares that
    version, so it expires on its own instead of becoming a switch left on."""
    if not BREAKAGE.is_file():
        return False
    declaration = json.loads(BREAKAGE.read_text(encoding="utf-8"))
    if not str(declaration.get("reason", "")).strip():
        fail(f"{BREAKAGE} names no reason, so nothing here says what broke")
    return declaration.get("version") == declared


def candidate_surface(explicit: str | None) -> pathlib.Path:
    if explicit:
        return pathlib.Path(explicit)
    completed = subprocess.run([sys.executable, str(SURFACE)], capture_output=True, text=True, check=True)
    path = pathlib.Path(tempfile.mkstemp(prefix="candidate-surface-", suffix=".json")[1])
    path.write_text(completed.stdout, encoding="utf-8")
    return path


def versions_declared_in_history() -> set[str]:
    """Every version an earlier commit on this branch committed to Cargo.toml.

    The manifest history is what the release procedure leaves behind: a
    coordinate is committed, then submitted. A coordinate committed here and
    then abandoned by a failed build is exactly the one a later version may
    skip.
    """
    log = subprocess.run(
        ["git", "-C", str(REPOSITORY), "log", "--format=%H", "--", "Cargo.toml"],
        capture_output=True, text=True, check=True,
    ).stdout.split()
    if len(log) <= 1:
        fail("the manifest history is one commit deep; fetch the full history before comparing versions")
    versions = set()
    for commit in log[1:]:
        shown = subprocess.run(
            ["git", "-C", str(REPOSITORY), "show", f"{commit}:Cargo.toml"],
            capture_output=True, text=True, check=False,
        )
        match = CARGO_VERSION.search(shown.stdout)
        if match:
            versions.add(match.group(1))
    return versions


def consumed_between(required: str, declared: str) -> list[str] | None:
    """The coordinates in the required version's series that the declared
    version skips, or None when the declared version is not in that series."""
    req, dec = parse(required), parse(declared)
    if dec[:2] != req[:2] or dec[2] <= req[2]:
        return None
    return [f"{req[0]}.{req[1]}.{patch}" for patch in range(req[2], dec[2])]


def compare(declared: str, released: str, change: str, required: str, history: set[str]) -> str:
    if declared == released:
        if change != INTERNAL:
            fail(f"Cargo.toml commits {declared}, but the public surface is {change} and requires {required}.")
        return f"Committed manifest version {declared} satisfies the {change} public-surface change from {released}."
    if declared == required:
        return f"Committed manifest version {declared} satisfies the {change} public-surface change from {released}."
    skipped = consumed_between(required, declared)
    if skipped is None:
        fail(f"Cargo.toml commits {declared}, but the {change} change from {released} requires {required}.")
    for coordinate in skipped:
        if coordinate not in history:
            fail(
                f"Cargo.toml commits {declared}, but the {change} change from {released} requires {required}, "
                f"and {coordinate} was never declared by an earlier commit on this branch, so nothing consumed it."
            )
    return (
        f"Committed manifest version {declared} satisfies the {change} public-surface change from {released}: "
        f"{required} is required and {', '.join(skipped)} were consumed by earlier commits."
    )


def self_check(autoversion: str, released: str) -> None:
    """Prove the rule and this comparison can both refuse before trusting either."""
    baseline = json.loads(BASELINE.read_text(encoding="utf-8"))
    with tempfile.TemporaryDirectory() as scratch:
        identical = pathlib.Path(scratch, "identical.json")
        identical.write_text(json.dumps({"surface": baseline["surface"]}), encoding="utf-8")
        removed = pathlib.Path(scratch, "removed.json")
        removed.write_text(json.dumps({"surface": sorted(baseline["surface"])[1:]}), encoding="utf-8")
        unchanged = decide(autoversion, released, identical)["change"]
        broken = decide(autoversion, released, removed)["change"]
    if unchanged != INTERNAL or broken != BREAKING:
        fail(f"AutoVersion self-check returned identical={unchanged} removed={broken}.")
    major, minor, patch = parse(released)
    required = f"{major}.{minor}.{patch + 1}"
    far = f"{major}.{minor}.{patch + 3}"
    try:
        compare(far, released, INTERNAL, required, history=set())
    except Refusal:
        return
    fail(f"the comparison accepted {far} against a required {required} with nothing consuming the gap.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--autoversion", required=True, help="the installed autoversion executable")
    parser.add_argument("--candidate", help="a candidate surface JSON; otherwise surface.py is run")
    parser.add_argument("--self-check", action="store_true", help="also prove the gate can refuse")
    arguments = parser.parse_args()
    declared = declared_version()
    released = released_version()
    if arguments.self_check:
        self_check(arguments.autoversion, released)
    verdict = decide(
        arguments.autoversion, released, candidate_surface(arguments.candidate), declared_breakage(declared)
    )
    print(json.dumps(verdict))
    print(compare(declared, released, verdict["change"], verdict["next"], versions_declared_in_history()))


if __name__ == "__main__":
    try:
        main()
    except Refusal as refusal:
        print(f"::error::{refusal}", file=sys.stderr)
        raise SystemExit(1)
