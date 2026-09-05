"""Regenerate released-surface.json from Brama's independent release channel.

Preference order, best first:

  github-release:<tag>  a GitHub Release with every supported archive and checksum
  git-archive:<tag>     a SemVer tag whose tree declares the tagged version
  head:<sha>            no usable release or tag exists

Usage:
    python3 scripts/baseline.py
    python3 scripts/baseline.py --write
    python3 scripts/baseline.py --declared-version
"""

from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ZERO = int(False)
ONE = int(True)

PRODUCT = "brama"
GITHUB_RELEASE_MARKER = "github-release:"
GIT_ARCHIVE_MARKER = "git-archive:"
HEAD_MARKER = "head:"
SUPPORTED_PLATFORMS = ("linux-amd64", "darwin-arm64")

REPOSITORY = pathlib.Path(__file__).resolve().parent.parent
BASELINE = REPOSITORY / "released-surface.json"
CARGO_VERSION = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)
SEMVER_TAG = re.compile(r"^v?(\d+\.\d+\.\d+)$")
GITHUB_REMOTE = re.compile(r"github\.com[:/]([^/]+/[^/]+?)(?:\.git)?$")


def scanner():
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


def gh(*arguments: str) -> object:
    executable = shutil.which("gh")
    if executable is None:
        raise LookupError("GitHub CLI is required to inspect Brama's release channel")
    output = subprocess.run(
        [executable, *arguments],
        cwd=REPOSITORY,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(output)


def repository_slug() -> str:
    configured = os.environ.get("GITHUB_REPOSITORY", "").strip()
    if configured:
        return configured
    remote = git("remote", "get-url", "origin").strip()
    match = GITHUB_REMOTE.search(remote)
    if match is None:
        raise LookupError(f"origin is not a GitHub repository URL: {remote}")
    return match.group(ONE)


def revision(reference: str) -> tuple[list[str], str]:
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


def honest_tag() -> str:
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
            print(f"ignoring tag {tag}: its tree declares {declared}", file=sys.stderr)
            continue
        order = tuple(int(part) for part in claimed.group(ONE).split("."))
        if best is None or order > best[ZERO]:
            best = (order, tag)
    return best[ONE] if best is not None else ""


def published_release() -> str:
    slug = repository_slug()
    releases = gh(
        "release",
        "list",
        "--repo",
        slug,
        "--limit",
        "100",
        "--json",
        "tagName,isDraft,isPrerelease",
    )
    candidates = []
    for release in releases:
        tag = release["tagName"]
        match = SEMVER_TAG.match(tag)
        if match is None or release["isDraft"] or release["isPrerelease"]:
            continue
        order = tuple(int(part) for part in match.group(ONE).split("."))
        candidates.append((order, tag))
    if not candidates:
        return ""

    details_cache: dict = {}
    for _, tag in sorted(candidates, reverse=True):
        claimed = SEMVER_TAG.match(tag)
        _, declared = revision(tag)
        if claimed is None or declared != claimed.group(ONE):
            # A release whose tree declares a different version than its name is
            # filed under a coordinate it does not contain. Believing the name
            # would measure every later change against the wrong artifact, so it
            # is reported and skipped — never allowed to abort the ladder, because
            # one mis-signed release would then freeze the baseline forever.
            print(
                f"baseline.py: release {tag} names version {claimed.group(ONE) if claimed else tag}"
                f" but its tree declares {declared}; skipping it and looking further back.",
                file=sys.stderr,
            )
            continue

        details = details_cache.setdefault(
            tag, gh("release", "view", tag, "--repo", slug, "--json", "assets")
        )
        asset_names = {asset["name"] for asset in details["assets"]}
        expected = {
            name
            for platform in SUPPORTED_PLATFORMS
            for name in (
                f"{PRODUCT}-{tag}-{platform}.tar.gz",
                f"{PRODUCT}-{tag}-{platform}.tar.gz.sha256",
            )
        }
        missing = sorted(expected - asset_names)
        if missing:
            print(
                f"baseline.py: release {tag} is incomplete; missing assets: "
                f"{', '.join(missing)}; skipping it and looking further back.",
                file=sys.stderr,
            )
            continue
        return tag
    return ""


def baseline() -> dict:
    release = published_release()
    if release:
        surface, declared = revision(release)
        return {
            "version": declared,
            "source": (
                f"{GITHUB_RELEASE_MARKER}{release} -- immutable archives and checksums "
                "published by the repository release workflow"
            ),
            "surface": surface,
        }

    tag = honest_tag()
    if tag:
        surface, declared = revision(tag)
        return {
            "version": declared,
            "source": (
                f"{GIT_ARCHIVE_MARKER}{tag} -- no complete GitHub Release exists; "
                "this tag is the best recoverable source baseline"
            ),
            "surface": surface,
        }

    head = git("rev-parse", "HEAD").strip()
    surface, declared = revision(head)
    return {
        "version": declared,
        "source": (
            f"{HEAD_MARKER}{head} -- NOT PUBLISHED: no complete GitHub Release or "
            "usable SemVer tag exists"
        ),
        "surface": surface,
    }


def declared_version() -> str:
    found = CARGO_VERSION.search((REPOSITORY / "Cargo.toml").read_text(encoding="utf-8"))
    if found is None:
        raise LookupError("Cargo.toml declares no package version")
    return found.group(ONE)


def main(argv: list[str]) -> int:
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
