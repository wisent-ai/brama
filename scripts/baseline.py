"""Regenerate released-surface.json: the surface of the version actually published.

A baseline is only worth comparing against if it describes something a consumer can
really have. So this never reads the working tree's declared version: it resolves the
best recoverable source in the fleet's preference order and records which one it used, in
a machine-readable marker that the version-check workflow asserts both ways.

Tiers, best first:

  stado:<object uri>   an object really present under stado://releases/brama/. This is
                       what `.github/workflows/deploy-stado.yml` publishes, and the only
                       artifact any consumer of this product can obtain.
  git-archive:<tag>    a tag whose own Cargo.toml declares the version the tag claims.
                       A tag that names a version its tree does not declare is refused,
                       not trusted, because a mis-signed tag would silently move the
                       baseline to a surface that was never released under that number.
  head:<sha>           last resort: nothing is published and no usable tag exists. The
                       baseline then honestly claims no publication at all.

The Stado release path carries the commit sha as its version component (see
`STADO_RELEASE_VERSION: ${{ github.sha }}` in the deploy workflow), so a published
release names the exact revision it was built from and the surface can be recovered by
materializing that revision with `git archive`. The alternative -- running `bin/brama
--help` out of the tarball -- only works on a linux host of the release's architecture,
so it is not required here; if the revision is unreachable this refuses and says so
rather than falling back to a lower tier.

Usage:
    python3 scripts/baseline.py            # print the baseline document
    python3 scripts/baseline.py --write    # rewrite released-surface.json in place
    python3 scripts/baseline.py --declared-version   # the version Cargo.toml declares
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

ZERO = int(False)
ONE = int(True)

PRODUCT = "brama"
NAMESPACE = "releases"
STADO_MARKER = "stado:"
GIT_ARCHIVE_MARKER = "git-archive:"
HEAD_MARKER = "head:"

REPOSITORY = pathlib.Path(__file__).resolve().parent.parent
BASELINE = REPOSITORY / "released-surface.json"
CARGO_VERSION = re.compile(r"^version\s*=\s*\"([^\"]+)\"", re.MULTILINE)
SEMVER_TAG = re.compile(r"^v?(\d+\.\d+\.\d+)$")
COMMIT_SHA = re.compile(r"^[0-9a-f]{7,40}$")


def scanner():
    """The surface scanner, so both files agree on what a command is."""
    path = pathlib.Path(__file__).resolve().parent / "surface.py"
    spec = importlib.util.spec_from_file_location("brama_surface", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def git(*arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=REPOSITORY,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def published_objects() -> list:
    """Every object Stado really serves for this product.

    A missing or unusable Stado CLI is an error, never an empty answer: silently
    treating "cannot ask" as "nothing published" is exactly how a baseline ends up
    claiming no release exists while consumers are installing one.
    """
    stado = shutil.which("stado")
    if stado is None:
        raise LookupError(
            "no `stado` on PATH, so publication cannot be determined; install the "
            "pinned Stado CLI as .github/workflows/deploy-stado.yml does"
        )
    listed = subprocess.run(
        [stado, "storage", "objects", NAMESPACE, f"{PRODUCT}/", "--json"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(listed)["objects"]


def revision(reference: str) -> tuple:
    """The command surface and declared version of one committed revision."""
    surface = scanner()
    workspace = tempfile.mkdtemp(prefix="brama-baseline-")
    try:
        archive = subprocess.run(
            ["git", "archive", "--format=tar", reference],
            cwd=REPOSITORY,
            check=True,
            capture_output=True,
        ).stdout
        subprocess.run(
            ["tar", "-x", "-C", workspace],
            input=archive,
            check=True,
            capture_output=True,
        )
        tree = pathlib.Path(workspace)
        declared = CARGO_VERSION.search((tree / "Cargo.toml").read_text(encoding="utf-8"))
        if declared is None:
            raise LookupError(f"{reference} declares no package version in Cargo.toml")
        return surface.declared_commands(tree), declared.group(ONE)
    finally:
        shutil.rmtree(workspace, ignore_errors=True)


def from_stado(objects: list) -> dict:
    """The baseline recovered from the most recently published release object."""
    newest = max(objects, key=lambda entry: entry["updated_at"])
    released = newest["key"].split("/")[ONE]
    if COMMIT_SHA.match(released) is None:
        raise LookupError(
            f"published release {released!r} is not a commit sha; recover its surface "
            "by running `bin/brama --help` from the published artifact instead"
        )
    try:
        git("cat-file", "-e", f"{released}^{{commit}}")
    except subprocess.CalledProcessError as unreachable:
        raise LookupError(
            f"published release {released} is not a revision this checkout can reach; "
            "fetch it, or recover the surface from the published artifact"
        ) from unreachable
    surface, declared = revision(released)
    return {
        "version": declared,
        "source": (
            f"{STADO_MARKER}{newest['uri']} -- commands read from revision {released}, "
            "which the release path names as the source of that artifact"
        ),
        "surface": surface,
    }


def honest_tag() -> str:
    """The newest tag whose own tree declares the version the tag claims.

    A tag naming a version its tree does not declare is reported and skipped, so a
    mis-signed tag cannot move the baseline onto a surface never released under that
    number. Alias tags for one version (`0.1.0` beside `v0.1.0`) tie, and the tie is
    broken by `git tag --list` order so the answer is stable across runs -- the
    workflow's staleness check needs this to converge, not merely to be defensible.
    """
    best = None
    for line in git("tag", "--list").splitlines():
        tag = line.strip()
        claimed = SEMVER_TAG.match(tag)
        if claimed is None:
            continue
        try:
            _, declared = revision(tag)
        except (subprocess.CalledProcessError, LookupError):
            continue
        if declared != claimed.group(ONE):
            print(
                f"ignoring tag {tag}: its tree declares {declared}",
                file=sys.stderr,
            )
            continue
        order = tuple(int(part) for part in claimed.group(ONE).split("."))
        if best is None or order > best[ZERO]:
            best = (order, tag)
    return best[ONE] if best is not None else ""


def baseline() -> dict:
    objects = published_objects()
    if objects:
        return from_stado(objects)

    tag = honest_tag()
    if tag:
        surface, declared = revision(tag)
        return {
            "version": declared,
            "source": (
                f"{GIT_ARCHIVE_MARKER}{tag} -- nothing is published under "
                f"stado://{NAMESPACE}/{PRODUCT}/, so the newest tag whose tree declares "
                "its own version is the best recoverable baseline"
            ),
            "surface": surface,
        }

    head = git("rev-parse", "HEAD").strip()
    surface, declared = revision(head)
    return {
        "version": declared,
        "source": (
            f"{HEAD_MARKER}{head} -- NOT PUBLISHED: stado://{NAMESPACE}/{PRODUCT}/ is "
            "empty and this repository has no tags, so this baseline claims no release "
            "and only records the contract as of that commit"
        ),
        "surface": surface,
    }


def declared_version() -> str:
    """The version this working tree declares, which is what a release would carry.

    Kept here so the workflow never parses Cargo.toml itself: one reader, used by both
    the baseline recovery and the gate, cannot drift from the other.
    """
    found = CARGO_VERSION.search((REPOSITORY / "Cargo.toml").read_text(encoding="utf-8"))
    if found is None:
        raise LookupError("Cargo.toml declares no package version")
    return found.group(ONE)


def main(argv: list) -> int:
    if "--declared-version" in argv:
        print(declared_version())
        return ZERO

    document = json.dumps(baseline(), indent=ONE + ONE) + "\n"
    if "--write" in argv:
        BASELINE.write_text(document, encoding="utf-8")
        print(f"wrote {BASELINE.name}", file=sys.stderr)
    else:
        sys.stdout.write(document)
    return ZERO


if __name__ == "__main__":
    sys.exit(main(sys.argv[ONE:]))
