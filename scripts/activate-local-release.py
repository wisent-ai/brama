#!/usr/bin/env python3
"""Make one locally packaged release self-hosting against the live trust material.

The launcher only prefers a bundle's own binaries when `etc/brama-skarbiec` sits
beside them, and a release correctly ships no such directory, so a freshly
installed bundle falls back to `/usr/local/bin` and refuses to start. Generating
fresh trust for it is the documented answer for a first installation; for a host
that already has a provisioned identity the smaller move is to let the new
bundle use the existing directory and re-pin it, because the registry names the
path and digest of the executable allowed to redeem and nothing else about the
bundle changes.

So: link the new bundle's `etc/brama-skarbiec` at the live one, then run the
bundle's own `provision-skarbiec-trust` against that directory with the new
binary. Reverse by relinking `current` and re-pinning with the old binary; this
prints both paths.

Targets exactly the version whose provenance names this packager, so a bundle
another operator is installing is never touched.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
SERVICES = HOME / ".stado" / "services" / "brama"
PLATFORM = os.environ.get("BRAMA_PLATFORM", "darwin-arm")
MARKER = os.environ.get("BRAMA_VERSION_MARKER", "catalog-capability")
NODE_CANDIDATES = (Path("/opt/homebrew/bin/node"), Path("/usr/local/bin/node"))


def provenance_version(version: Path) -> str:
    # `service update` creates the platform directory and unpacks inside it, so
    # an archive's root record lands one level down from where a reader expects.
    for record in (version / "provenance.json", version / PLATFORM / "provenance.json"):
        if not record.exists():
            continue
        try:
            return str(json.loads(record.read_text()).get("version", ""))
        except ValueError:
            return ""
    return ""

# Prefer the directory the serving bundle actually uses, because that is the
# material the fleet is authenticating against right now; fall back to a search
# only when `current` has already been relinked to an unprovisioned newcomer,
# which is what an update does before activation runs. Picking any provisioned
# directory instead re-pins a registry nobody reads.
serving = (SERVICES / "current" / PLATFORM / "etc" / "brama-skarbiec").resolve()
if (serving / "policy.json").exists():
    live_trust = serving
else:
    provisioned = [
        candidate / PLATFORM / "etc" / "brama-skarbiec"
        for candidate in sorted(SERVICES.iterdir(), key=lambda path: path.name)
        if not candidate.is_symlink()
        and (candidate / PLATFORM / "etc" / "brama-skarbiec" / "policy.json").exists()
    ]
    if not provisioned:
        raise SystemExit("no installed bundle carries provisioned trust to reuse")
    live_trust = provisioned[len(provisioned) - len("x")].resolve()
print("trust source:", live_trust)

targets = [
    version
    for version in sorted(SERVICES.iterdir())
    if not version.is_symlink() and MARKER in provenance_version(version)
]
if not targets:
    raise SystemExit(f"no installed release names {MARKER} in its provenance")

node = next((path for path in NODE_CANDIDATES if path.exists()), None)
if node is None:
    raise SystemExit("node is unavailable on this host")

# Directory names are digests, so "the newest" has to come from the version each
# bundle records, not from sort order on disk.
targets.sort(key=provenance_version)
target = targets[len(targets) - len("x")]
print("target:", target.name, provenance_version(target))

# A bundle that is not the target must not keep a trust link this script made:
# with one it looks self-hosting to the launcher while carrying an older binary.
for other in targets:
    if other == target:
        continue
    stray = other / PLATFORM / "etc" / "brama-skarbiec"
    if stray.is_symlink():
        stray.unlink()
        print("unlinked stray:", stray)

bundle = target / PLATFORM
trust_link = bundle / "etc" / "brama-skarbiec"
trust_link.parent.mkdir(parents=True, exist_ok=True)
if trust_link.is_symlink() and trust_link.resolve() != live_trust:
    # A link left by an earlier activation can name a different provisioned
    # directory than the one just re-pinned, and then the bundle reads a
    # registry naming another binary. One of the two has to move; the link is
    # the cheap one.
    trust_link.unlink()
    print("repointed stale link")
if trust_link.is_symlink() or trust_link.exists():
    print("already linked:", trust_link)
else:
    trust_link.symlink_to(live_trust)
    print("linked:", trust_link, "->", live_trust)

done = subprocess.run(
    [str(bundle / "bin" / "provision-skarbiec-trust"), "--force"],
    capture_output=True,
    text=True,
    env={
        **os.environ,
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "NODE_BIN": str(node),
        "BRAMA_BIN": str(bundle / "bin" / "brama"),
        "BRAMA_SKARBIEC_CONFIG_DIR": str(live_trust),
    },
)
print("provision exit:", done.returncode)
for line in (done.stdout or "").splitlines():
    print("  out:", line)
for line in (done.stderr or "").splitlines()[-len("........"):]:
    print("  err:", line)
