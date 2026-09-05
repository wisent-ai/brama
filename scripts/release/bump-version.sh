#!/usr/bin/env bash
# Move brama to one exact version everywhere a release reads it.
#
# A version lives in four places and a release refuses any disagreement:
# Cargo.toml declares it, Cargo.lock and Cargo.release.lock pin the package the
# `--locked` release build compiles, and `.wisent-release.json` must name the
# version this one replaces under `runtime.rollback_compatible_with` or every
# rollout target quarantines the digest. On 2026-09-05 three release coordinates
# were spent one after another because each bump touched a different subset:
# 0.2.73 on a missing quality script, then 0.2.74 on `Cargo.release.lock` still
# pinning 0.2.72 — and a release object that has attested a source revision is
# immutable, so each miss costs the whole version.
#
# Usage: scripts/release/bump-version.sh <version>
set -euo pipefail

version=${1:?exact SemVer to declare, without a leading v}
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf '%s is not an exact SemVer\n' "$version" >&2
  exit 64
fi

repository=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repository"

for required in Cargo.toml Cargo.lock Cargo.release.lock .wisent-release.json; do
  if [[ ! -f "$required" ]]; then
    printf 'this checkout is missing %s\n' "$required" >&2
    exit 66
  fi
done

python3 - "$version" <<'PY'
import json
import pathlib
import re
import sys

version = sys.argv[1]
manifest = pathlib.Path("Cargo.toml")
declared = re.search(r'^version = "([^"]+)"', manifest.read_text(), re.M)
if declared is None:
    raise SystemExit("Cargo.toml declares no version")
previous = declared.group(1)
if previous == version:
    print(f"Cargo.toml already declares {version}")
else:
    manifest.write_text(
        re.sub(
            r'^version = "[^"]+"',
            f'version = "{version}"',
            manifest.read_text(),
            count=1,
            flags=re.M,
        )
    )
    print(f"Cargo.toml {previous} -> {version}")

package = re.compile(r'(\[\[package\]\]\nname = "brama"\nversion = ")[^"]+(")', re.M)
for lock in ("Cargo.lock", "Cargo.release.lock"):
    path = pathlib.Path(lock)
    body = path.read_text()
    pinned = package.search(body)
    if pinned is None:
        raise SystemExit(f"{lock} carries no brama package entry")
    held = re.search(
        r'\[\[package\]\]\nname = "brama"\nversion = "([^"]+)"', body, re.M
    ).group(1)
    if held == version:
        print(f"{lock} already pins {version}")
        continue
    path.write_text(package.sub(rf"\g<1>{version}\g<2>", body, count=1))
    print(f"{lock} {held} -> {version}")

release = pathlib.Path(".wisent-release.json")
document = json.loads(release.read_text())
compatible = document["runtime"]["rollback_compatible_with"]
if previous != version and previous not in compatible:
    compatible.insert(0, previous)
    release.write_text(json.dumps(document, indent=2) + "\n")
    print(f".wisent-release.json declares rollback to {previous}")
else:
    print(f".wisent-release.json already declares rollback to {previous}")
PY

printf 'run `cargo build --bin brama` and commit Cargo.toml, both locks and .wisent-release.json together\n'
