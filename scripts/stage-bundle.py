#!/usr/bin/env python3
"""Stage the release bundle layout from an already-built tree.

The release workflow assembles this from CI. An operator repairing a host needs
the same layout from the same sources without waiting for a publication, and
assembling it by hand is how a bundle ends up missing the one file that lets an
installation provision itself. So the layout lives here, once, and the workflow
and the operator both get it from the same description.

    stage-bundle.py <brama-binary> <router-binary> <output.tar.gz>

Trust material is deliberately absent: it pins the path, digest and account of
the installation that will run, none of which is known here.
"""

import hashlib
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile

arguments = iter(sys.argv)
next(arguments)
try:
    brama_binary, router_binary, output = arguments
except ValueError:
    raise SystemExit("usage: stage-bundle.py <brama-binary> <router-binary> <output.tar.gz>")

repository = pathlib.Path(__file__).resolve().parent.parent
scripts = repository / "scripts"

EXECUTABLES = {
    "bin/brama": pathlib.Path(brama_binary),
    "bin/skarbiec-entitlements-router": pathlib.Path(router_binary),
    "bin/start-with-skarbiec": scripts / "start-with-skarbiec.sh",
    "bin/provision-skarbiec-trust": scripts / "provision-skarbiec-trust.sh",
}
DATA = {
    "libexec/generate-skarbiec-config.mjs": scripts / "generate-skarbiec-config.mjs",
    "libexec/brama-register-workload.py": scripts / "brama-register-workload.py",
    "libexec/brama-write-capability-routes.py": scripts / "brama-write-capability-routes.py",
    "libexec/brama-diagnose.py": scripts / "brama-diagnose.py",
    "libexec/brama-route-probe.py": scripts / "brama-route-probe.py",
    "libexec/brama-clear-stale-broker.py": scripts / "brama-clear-stale-broker.py",
    "libexec/brama-repair-inference-routes.py": scripts / "brama-repair-inference-routes.py",
    "etc/brama-skarbiec/subscriptions.json": scripts / "skarbiec-subscriptions.json",
    "etc/brama-skarbiec/recipient-public-keys.asc": scripts / "skarbiec-recipient-public-keys.asc",
    "LICENSE": repository / "LICENSE",
}

with tempfile.TemporaryDirectory() as workspace:
    # The archive carries `bin/`, `libexec/` and `etc/` at its root, exactly as
    # the release workflow's `tar -czf … -C "$stage" .` does. The installer on
    # the far side creates the platform directory itself, so an archive that
    # also contains one produces `darwin-arm/darwin-arm/bin/brama`: the unit's
    # program path does not exist, launchd reports that to the system log rather
    # than to the service's own, and every report about the service looks
    # healthy while nothing runs.
    stage = pathlib.Path(workspace) / "stage"
    for relative, source in {**EXECUTABLES, **DATA}.items():
        if not source.is_file():
            raise SystemExit(f"missing {source}")
        destination = stage / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
        if relative in EXECUTABLES:
            destination.chmod(source.stat().st_mode | pathlib.Path(brama_binary).stat().st_mode)
    revision = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    (stage / "provenance.json").write_text(
        f'{{"product":"brama","source_revision":"{revision}","platform":"darwin-arm64",'
        f'"builder":"operator-local"}}\n'
    )
    with tarfile.open(output, "w:gz") as archive:
        archive.add(stage, arcname=".")

digest = hashlib.sha256(pathlib.Path(output).read_bytes()).hexdigest()
print(f"{output}")
print(f"sha256 {digest}")
print(f"source_revision {revision}")
