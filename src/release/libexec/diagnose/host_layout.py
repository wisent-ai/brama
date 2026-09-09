"""Where this host keeps Brama, and what its own files say about that.

Every section of the diagnostic compares one file against another. This module
answers only the prior question - which files those are on this machine: the
service directories, the env the unit was handed, the release state naming the
generation that actually serves, and the generations installed beside it. It
prints nothing, so that the sections that do print read as comparisons.
"""

import hashlib
import json
import os
import pathlib
import time

SERVICE_LABEL = "com.wisent.always-on.brama"
CAPABILITY_VERB = "capability-issue"
REFUSAL = "unknown command"
BOOT_MARKER = "Starting server"
BEST_ALIAS = "best"
PROVIDER_PURPOSE = "brama.provider.authenticate"
REQUIRED_FILES = (
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
    "bin/provision-skarbiec-trust",
    "libexec/generate-skarbiec-config.mjs",
)

home = pathlib.Path.home()
services = home / ".stado" / "services" / "brama"
env_file = home / ".config" / "brama" / "service.env"
error_log = home / ".stado" / "logs" / "brama-always-on.err"
output_log = home / ".stado" / "logs" / "brama-always-on.out"
uid = os.getuid()
gid = os.getgid()


def moment(epoch):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(epoch))


def normalize(provider):
    """The provider spelling used in an authority route."""
    return provider.strip().lower().replace("_", "-")


def settings_of(path):
    values = {}
    if not path.is_file():
        return values
    for line in path.read_text(errors="replace").splitlines():
        name, separator, value = line.partition("=")
        if separator and not name.lstrip().startswith("#"):
            values[name.strip()] = value.strip().strip("'\"")
    return values


settings = settings_of(env_file)
release_state_path = home / ".stado" / "release-state" / "brama.json"
try:
    release_state = json.loads(release_state_path.read_text())
except (OSError, ValueError):
    release_state = {}


def release_record(name):
    record = release_state.get(name)
    if not isinstance(record, dict):
        return None
    release_dir = record.get("release_dir")
    return record if isinstance(release_dir, str) and release_dir else None


def record_root(name):
    record = release_record(name)
    return pathlib.Path(record["release_dir"]).resolve() if record else None


active_record = release_record("active")
release_roots = {
    name: root
    for name in ("active", "candidate", "previous")
    if (root := record_root(name)) is not None
}


def installed_generations():
    if not services.is_dir():
        return []
    direct = [
        path
        for path in services.iterdir()
        if path.name != "releases" and not path.is_symlink() and path.is_dir()
    ]
    releases = services / "releases"
    nested = (
        [path for path in releases.iterdir() if not path.is_symlink() and path.is_dir()]
        if releases.is_dir()
        else []
    )
    return sorted(direct + nested, key=lambda path: (path.stat().st_mtime, str(path)))


def config_dir_for(generation):
    return home / ".config" / "brama" / f"trust-{generation.name}"


def architecture_root(generation):
    for platform in ("darwin-arm64", "darwin-arm"):
        nested = generation / platform
        if nested.is_dir():
            return nested
    return generation


def digest_of(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else "missing"
