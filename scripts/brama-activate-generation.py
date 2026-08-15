#!/usr/bin/env python3
"""Give the newest self-sufficient Brama installation an identity and run it.

The order here is the one RELEASE.md names, and it is the whole point:
provision first, confirm the workload registry describes THIS installation,
and only then repoint the service manager. Repointing first is what leaves a
gateway that answers /health and serves nothing.

The generation is chosen by property rather than by name: the newest
installation that ships its own provisioner, generator and subscriptions
manifest, because those are exactly what an installation needs to hold an
identity of its own instead of borrowing another one's.

Everything replaced is saved beside the original and the undo is printed.
"""

import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

home = pathlib.Path.home()
services = home / ".stado" / "services" / "brama"
env_file = home / ".config" / "brama" / "service.env"

settings = {}
if env_file.is_file():
    for line in env_file.read_text(errors="replace").splitlines():
        name, separator, value = line.partition("=")
        if separator and not name.lstrip().startswith("#"):
            settings[name.strip()] = value.strip().strip("'\"")

REQUIRED = (
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
    "bin/provision-skarbiec-trust",
    "libexec/generate-skarbiec-config.mjs",
)


def architecture_root(generation):
    nested = generation / "darwin-arm"
    return nested if nested.is_dir() else generation


def router_answers(root):
    """True when this generation's router implements the verbs the launcher calls.

    The capability verbs are dispatched ahead of the documented command table,
    so `--help` omits them and reading it answers no for a router that has
    them. Invoking one answers: a router without it says `unknown command`.
    The vault is pointed at a path that does not exist, so nothing is touched.
    """
    router = root / "bin" / "skarbiec-entitlements-router"
    if not router.is_file():
        return False
    probe = dict(os.environ)
    probe["SKARBIEC_VAULT_FILE"] = str(root / "no-such-vault.json")
    answered = subprocess.run(
        [str(router), "capability-issue"],
        capture_output=True,
        text=True,
        check=False,
        env=probe,
    )
    return "unknown command" not in (answered.stdout + answered.stderr)


def self_sufficient(generation):
    root = architecture_root(generation)
    return all((root / name).exists() for name in REQUIRED) and router_answers(root)


candidates = [
    generation
    for generation in services.iterdir()
    if generation.is_dir() and not generation.is_symlink() and self_sufficient(generation)
]
if not candidates:
    raise SystemExit(
        "no installed generation both ships its provisioning material and has a "
        "router that answers the launcher"
    )

chosen = max(candidates, key=lambda generation: generation.stat().st_mtime)
root = architecture_root(chosen)
binary = root / "bin" / "brama"
config_dir = root / "etc" / "brama-skarbiec"
print(f"generation: {chosen.name}")

node = shutil.which("node") or "/opt/homebrew/bin/node"
if not pathlib.Path(node).is_file():
    raise SystemExit("node is required to sign the policy and registry")

environment = dict(os.environ)
environment.update(
    {
        "NODE_BIN": node,
        "BRAMA_BIN": str(binary),
        "BRAMA_SKARBIEC_CONFIG_DIR": str(config_dir),
        "BRAMA_WORKLOAD_UID": str(os.getuid()),
        "BRAMA_WORKLOAD_GID": str(os.getgid()),
    }
)
print(f"provisioning as uid={os.getuid()} gid={os.getgid()}")
subprocess.run(
    [str(root / "bin" / "provision-skarbiec-trust"), "--force"],
    env=environment,
    check=True,
)

# Confirm before repointing. A registry that still describes somebody else is
# precisely the failure this script exists to prevent.
workload = next(
    iter(json.loads((config_dir / "registry.json").read_text()).get("workloads", {}).values()),
    {},
)
expected = {
    "uid": os.getuid(),
    "gid": os.getgid(),
    "executable_path": str(binary),
    "executable_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
}
disagreements = [
    f"{name}: pinned={workload.get(name)} actual={value}"
    for name, value in expected.items()
    if str(workload.get(name)) != str(value)
]
if disagreements:
    for line in disagreements:
        print(f"registry still disagrees on {line}", file=sys.stderr)
    raise SystemExit("not repointing: the registry does not describe this installation")
print("registry describes this installation")

stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
backup = env_file.with_name(f"{env_file.name}.before-{chosen.name}-{stamp}")
shutil.copyfile(env_file, backup)
assignments = {
    "BRAMA_BIN": str(binary),
    "ENTITLEMENTS_ROUTER_BIN": str(root / "bin" / "skarbiec-entitlements-router"),
    "BRAMA_SKARBIEC_CONFIG_DIR": str(config_dir),
}
# Which address the gateway binds is not this script's decision and not the TLS
# terminator's either: the launcher asks Stado for the placement
# (`service directory bind brama`) and the registry's answer wins over anything
# written here. An assignment in the service env is therefore inert at best and
# misleading at worst, so it is removed rather than set.
stale = "BRAMA_BIND_ADDRESS"
kept = [
    line
    for line in env_file.read_text().splitlines()
    if not any(line.startswith(f"{name}=") for name in assignments)
    and not line.startswith(f"{stale}=")
]
kept.extend(f"{name}={value}" for name, value in assignments.items())
env_file.write_text("\n".join(kept) + "\n")
print(f"service env rewritten; previous copy: {backup}")

current = services / "current"
previous = os.readlink(current) if current.is_symlink() else ""
(home / ".stado" / "brama-previous-generation").write_text(f"{previous}\n")
temporary = services / "current.stado-activate"
if temporary.is_symlink() or temporary.exists():
    temporary.unlink()
temporary.symlink_to(chosen)
os.replace(temporary, current)
print(f"current: {previous} -> {os.readlink(current)}")
print("undo: run helper brama-restore-generation")
