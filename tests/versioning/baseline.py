"""Regenerate released-surface.json from Brama's release channel.

Preference order, best first:

  stado://releases/...  a release the Stado release plane has published and a
                        host is running; this is the channel `stado release
                        submit` writes and the one production installs from
  github-release:<tag>  a GitHub Release with every supported archive and checksum
  git-archive:<tag>     a SemVer tag whose tree declares the tagged version
  head:<sha>            no usable release or tag exists

The Stado channel comes first because it is the one that ships. GitHub Releases
stopped at v0.2.53 when the tag job stalled, while the release plane went on
publishing every version production runs — so measuring the surface against the
GitHub channel compared today's tree with a release from twenty-two versions
ago, and the contract gate demanded a version that had already been published.

Usage:
    python3 tests/versioning/baseline.py
    python3 tests/versioning/baseline.py --write
    python3 tests/versioning/baseline.py --declared-version
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import re
import sys

ZERO = int(False)
ONE = int(True)

GITHUB_RELEASE_MARKER = "github-release:"
GIT_ARCHIVE_MARKER = "git-archive:"
HEAD_MARKER = "head:"
STADO_RELEASE_MARKER = "stado://releases/"

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
BASELINE = REPOSITORY / "released-surface.json"
CARGO_VERSION = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)


def channels():
    """The release channels, loaded by path like every other script here."""
    path = pathlib.Path(__file__).resolve().parent / "channels.py"
    spec = importlib.util.spec_from_file_location("brama_channels", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def baseline() -> dict:
    channel = channels()
    version, revision_id, artifact = channel.stado_release()
    if version:
        surface, declared = channel.revision(revision_id)
        if declared != version:
            print(
                f"baseline.py: published {version} was built from {revision_id}, whose tree"
                f" declares {declared}; recording the tree's own version.",
                file=sys.stderr,
            )
        return {
            "version": declared,
            "source": (
                f"{artifact} source_revision={revision_id} -- signed release object the "
                "Stado release plane published and hosts install from"
            ),
            "surface": surface,
        }

    release = channel.published_release()
    if release:
        surface, declared = channel.revision(release)
        return {
            "version": declared,
            "source": (
                f"{GITHUB_RELEASE_MARKER}{release} -- immutable archives and checksums "
                "published by the repository release workflow"
            ),
            "surface": surface,
        }

    tag = channel.honest_tag()
    if tag:
        surface, declared = channel.revision(tag)
        return {
            "version": declared,
            "source": (
                f"{GIT_ARCHIVE_MARKER}{tag} -- no complete GitHub Release exists; "
                "this tag is the best recoverable source baseline"
            ),
            "surface": surface,
        }

    head = channel.git("rev-parse", "HEAD").strip()
    surface, declared = channel.revision(head)
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
