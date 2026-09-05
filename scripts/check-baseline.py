#!/usr/bin/env python3
"""Check released-surface.json against what this repository can prove.

The baseline is the surface of the release the contract gate measures every
change against, so a baseline nobody checks is a gate nobody can trust. What is
checkable depends on the channel that published it:

  stado://releases/...  the source revision is in this checkout, so recompute
                        the surface from that exact tree and compare, and
                        require the version to be one this tree knows -- the
                        manifest's own version or one it declares rollback
                        compatibility with
  github-release:<tag>  the GitHub channel is reachable from CI, so require the
                        baseline to name the newest complete release
  git-archive:<tag>     the tag is in this checkout: recompute and compare
  head:<sha>            nothing is published; the commit must be in this
                        checkout and its surface must match

Before this existed the job asserted one thing only -- that the marker was a
GitHub release and the newest one -- while the file recorded a Stado release,
which is the channel production installs from. The assertion could therefore
never pass: main stayed red, the tag job behind it never ran, and the GitHub
channel it was asserting about froze at v0.2.53 while twenty-two further
versions shipped.

Usage: python3 scripts/check-baseline.py
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

REPOSITORY = pathlib.Path(__file__).resolve().parent.parent
BASELINE = REPOSITORY / "released-surface.json"
MANIFEST = REPOSITORY / "Cargo.toml"
RELEASE = REPOSITORY / ".wisent-release.json"
CARGO_VERSION = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)
SOURCE_REVISION = re.compile(r"source_revision=([0-9a-f]{7,40})")
STADO_MARKER = "stado://releases/"
GITHUB_MARKER = "github-release:"
GIT_ARCHIVE_MARKER = "git-archive:"
HEAD_MARKER = "head:"


def generator():
    path = pathlib.Path(__file__).resolve().parent / "baseline.py"
    spec = importlib.util.spec_from_file_location("brama_baseline", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def refuse(message: str) -> int:
    print(f"::error::{message}")
    return 1


def declared_version() -> str:
    found = CARGO_VERSION.search(MANIFEST.read_text(encoding="utf-8"))
    if found is None:
        raise SystemExit("Cargo.toml declares no package version")
    return found.group(1)


def known_versions() -> set[str]:
    document = json.loads(RELEASE.read_text(encoding="utf-8"))
    compatible = document.get("runtime", {}).get("rollback_compatible_with", [])
    return {declared_version(), *compatible}


def surface_of(reference: str, tools) -> tuple[list[str], str]:
    return tools.revision(reference)


def main() -> int:
    if not BASELINE.is_file():
        return refuse("released-surface.json is missing")
    baseline = json.loads(BASELINE.read_text(encoding="utf-8"))
    source = str(baseline.get("source", ""))
    recorded_surface = baseline.get("surface")
    recorded_version = baseline.get("version")
    if not source or not isinstance(recorded_surface, list) or not recorded_version:
        return refuse("released-surface.json must carry source, version and surface")
    tools = generator()

    if source.startswith(STADO_MARKER):
        revision_id = SOURCE_REVISION.search(source)
        if revision_id is None:
            return refuse(
                "a stado:// baseline must record source_revision=<sha> in its source"
            )
        revision_id = revision_id.group(1)
        try:
            subprocess.run(
                ["git", "cat-file", "-e", f"{revision_id}^{{commit}}"],
                cwd=REPOSITORY,
                check=True,
                capture_output=True,
            )
        except subprocess.CalledProcessError:
            return refuse(
                f"the baseline names source revision {revision_id}, which this checkout"
                " does not hold; fetch it or regenerate with"
                " python3 scripts/baseline.py --write"
            )
        surface, declared = surface_of(revision_id, tools)
        if declared != recorded_version:
            return refuse(
                f"the baseline records version {recorded_version}, but {revision_id}"
                f" declares {declared}"
            )
        if surface != recorded_surface:
            return refuse(
                f"the baseline surface does not match the surface of {revision_id};"
                " regenerate it with python3 scripts/baseline.py --write"
            )
        if recorded_version not in known_versions():
            return refuse(
                f"the baseline records {recorded_version}, which this tree neither"
                " declares nor lists under runtime.rollback_compatible_with"
            )
        print(
            f"Baseline {recorded_version} matches the Stado release plane and the"
            f" surface of {revision_id}."
        )
        return 0

    if source.startswith(GITHUB_MARKER):
        best = tools.baseline()
        want = str(best.get("source", "")).split(" ")[0]
        have = source.split(" ")[0]
        if have != want:
            return refuse(
                f"the baseline is {have}, but {want} is the best complete release;"
                " regenerate it with python3 scripts/baseline.py --write"
            )
        print(f"Baseline {have} matches the release channel.")
        return 0

    if source.startswith(GIT_ARCHIVE_MARKER) or source.startswith(HEAD_MARKER):
        reference = source.split(" ")[0].split(":", 1)[1]
        try:
            surface, declared = surface_of(reference, tools)
        except (subprocess.CalledProcessError, LookupError):
            return refuse(f"the baseline names {reference}, which this checkout cannot read")
        if declared != recorded_version or surface != recorded_surface:
            return refuse(
                f"the baseline does not match {reference}; regenerate it with"
                " python3 scripts/baseline.py --write"
            )
        print(f"Baseline {recorded_version} matches {reference}.")
        return 0

    return refuse(f"unknown baseline marker: {source.split(' ')[0]}")


if __name__ == "__main__":
    sys.exit(main())
