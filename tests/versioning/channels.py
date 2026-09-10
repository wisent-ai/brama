"""The three channels a released surface can be measured against.

Best first: the Stado release plane, which is what production installs from;
a GitHub Release carrying every supported archive and checksum; and a SemVer
tag whose tree declares the tagged version. Each function here answers with
what that channel actually published, or an empty answer when it published
nothing usable — the choice between them belongs to `baseline.py`.
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
STADO_RELEASE_MARKER = "stado://releases/"
STADO_BIN = os.environ.get("STADO_BIN", "stado")
SUPPORTED_PLATFORMS = ("linux-amd64", "darwin-arm64")

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
CARGO_VERSION = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)
SEMVER_TAG = re.compile(r"^v?(\d+\.\d+\.\d+)$")
GITHUB_REMOTE = re.compile(r"github\.com[:/]([^/]+/[^/]+?)(?:\.git)?$")

# Throwaway trees are extracted inside this checkout's ignored build
# directory. A run must not leave anything outside the checkout it measures.
WORK_ROOT = REPOSITORY / "target" / "baseline-work"


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
    WORK_ROOT.mkdir(parents=True, exist_ok=True)
    workspace = tempfile.mkdtemp(prefix="brama-baseline-", dir=WORK_ROOT)
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


def stado_release() -> tuple[str, str, str]:
    """The newest published Stado release: version, source revision, artifact.

    `stado release status brama` answers with what the release plane published
    and what each host actually runs. A version is only usable as a baseline
    when its source revision is in this checkout, because the surface has to be
    recomputed from that exact tree rather than trusted from a file.

    `stado release status brama` exits non-zero whenever a rollout target
    cannot be shown to be running the declared version, which is a statement
    about the fleet and not about the answer: the report is on stdout either
    way. Treating that exit code as "no channel" is what made this reader fall
    through to a GitHub release twenty-two versions old.
    """
    executable = shutil.which(STADO_BIN)
    if executable is None:
        return ("", "", "")
    try:
        answer = subprocess.run(
            [executable, "release", "status", PRODUCT, "--json"],
            check=False,
            capture_output=True,
            text=True,
        ).stdout
        report = json.loads(answer)
    except (json.JSONDecodeError, OSError):
        return ("", "", "")
    published = []
    for target in report.get("targets") or []:
        desired = target.get("desired") or {}
        version = desired.get("version")
        if not version or not SEMVER_TAG.match(f"v{version}"):
            continue
        for platform, artifact in (desired.get("artifacts") or {}).items():
            revision_id = (artifact or {}).get("source_revision")
            manifest = (artifact or {}).get("manifest_uri") or ""
            if not revision_id or not manifest.startswith(STADO_RELEASE_MARKER):
                continue
            published.append(
                (
                    tuple(int(part) for part in version.split(".")),
                    version,
                    revision_id,
                    f"{STADO_RELEASE_MARKER}{PRODUCT}/{version}/{platform}",
                )
            )
    for _, version, revision_id, artifact in sorted(published, reverse=True):
        try:
            git("cat-file", "-e", f"{revision_id}^{{commit}}")
        except subprocess.CalledProcessError:
            print(
                f"baseline.py: published {version} names source revision {revision_id},"
                " which this checkout does not hold; looking further back.",
                file=sys.stderr,
            )
            continue
        return (version, revision_id, artifact)
    return ("", "", "")
