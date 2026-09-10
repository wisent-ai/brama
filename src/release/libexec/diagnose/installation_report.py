"""What is installed here, and whether this host was told about that exact copy.

The units name a path, the path leads to a generation, and the trust registry
pins the uid, gid, absolute path and SHA-256 of the binary allowed to redeem a
capability. A generation can be complete and still refuse to serve because the
registry beside it describes another installation, so each is printed next to
the thing it has to agree with rather than on its own.
"""

import json
import os
import pathlib
import plistlib
import subprocess

from host_layout import (
    CAPABILITY_COMMAND,
    REFUSAL,
    REQUIRED_FILES,
    SERVICE_LABEL,
    active_record,
    architecture_root,
    config_dir_for,
    digest_of,
    gid,
    home,
    installed_generations,
    moment,
    release_roots,
    services,
    settings,
    uid,
)


def router_answers(root):
    router = root / "bin" / "skarbiec-entitlements-router"
    if not router.is_file():
        return False
    probe = dict(os.environ)
    probe["SKARBIEC_VAULT_FILE"] = str(root / "no-such-vault.json")
    answered = subprocess.run(
        [str(router), *CAPABILITY_COMMAND],
        capture_output=True,
        text=True,
        check=False,
        env=probe,
    )
    return REFUSAL not in (answered.stdout + answered.stderr)


def registry_verdict(root, config_dir):
    registry = config_dir / "registry.json"
    if not registry.is_file():
        return [f"{registry}: absent"]
    try:
        document = json.loads(registry.read_text())
    except ValueError as failure:
        return [f"{registry}: unreadable: {failure}"]
    workload = next(iter(document.get("workloads", {}).values()), {})
    binary = root / "bin" / "brama"
    expected = {
        "uid": uid,
        "gid": gid,
        "executable_path": str(binary),
        "executable_sha256": digest_of(binary),
    }
    wrong = [
        f"{name} pinned={workload.get(name)} actual={value}"
        for name, value in expected.items()
        if str(workload.get(name)) != str(value)
    ]
    return wrong or [f"{registry}: describes this installation"]


def print_units():
    """The units that start Brama, and the release state that actually serves."""
    print("=== units that start Brama")
    current = services / "current"
    resolved = current.resolve() if current.exists() else None
    if current.is_symlink():
        print(f"current -> {os.readlink(current)} (link written {moment(current.lstat().st_mtime)})")
    unit_locations = [pathlib.Path("/Library/LaunchDaemons"), home / "Library" / "LaunchAgents"]
    for location in unit_locations:
        if not location.is_dir():
            continue
        for plist in sorted(location.glob("*.plist")):
            try:
                document = plistlib.loads(plist.read_bytes())
            except Exception:
                continue
            arguments = document.get("ProgramArguments", [])
            joined = " ".join(arguments)
            label = document.get("Label", "")
            if SERVICE_LABEL not in label and "start-with-skarbiec" not in joined:
                continue
            print(f"  {plist}")
            print(f"    label:    {label}")
            print(f"    program:  {joined}")
            for argument in arguments:
                candidate = pathlib.Path(argument)
                if candidate.is_file():
                    print(f"    leads to: {candidate.resolve()}")
                    break
    return resolved


def print_generations(resolved):
    """Every installed generation: completeness, router verbs, trust registry."""
    print("\n=== installed generations")
    for generation in installed_generations():
        root = architecture_root(generation)
        if not (root / "bin").is_dir():
            continue
        roles = [
            name
            for name, release_root in release_roots.items()
            if root.resolve() == release_root
        ]
        if generation.resolve() == resolved:
            roles.append("legacy current")
        marker = f" <- {', '.join(roles)}" if roles else ""
        missing = [name for name in REQUIRED_FILES if not (root / name).exists()]
        print(f"  {generation}{marker}  installed {moment(generation.stat().st_mtime)}")
        print(f"    files:    {'complete' if not missing else 'missing ' + ', '.join(missing)}")
        print(f"    router {' '.join(CAPABILITY_COMMAND)}: {router_answers(root)}")
        unrunnable = [
            name
            for name in ("bin/brama", "bin/skarbiec-entitlements-router", "bin/start-with-skarbiec")
            if (root / name).is_file() and not os.access(root / name, os.X_OK)
        ]
        if unrunnable:
            print(f"    NOT EXECUTABLE: {', '.join(unrunnable)}")
        for line in registry_verdict(root, config_dir_for(generation)):
            print(f"    registry: {line}")


def print_service_env(resolved):
    """The service env values beside the supervisor-owned runtime coordinates.

    Returns the trust config directory the later sections read, which is the one
    belonging to the generation that serves rather than the newest installed.
    """
    print("\n=== service env")
    for name in sorted(settings):
        if "TOKEN" in name or "SECRET" in name or "KEY" in name or "PASSWORD" in name:
            print(f"  {name}=<redacted>")
        else:
            print(f"  {name}={settings[name]}")

    if active_record:
        active_root = pathlib.Path(active_record["release_dir"]).resolve()
        active_generation = active_root.parent
        config_dir = config_dir_for(active_generation)
        runtime_dir = (
            home
            / ".stado"
            / "run"
            / "brama"
            / f"{active_record.get('version')}-{active_record.get('port')}"
        )
        print(
            f"active release: {active_record.get('version')} pid={active_record.get('pid')} "
            f"port={active_record.get('port')} root={active_root}"
        )
    else:
        generation = resolved.parent if resolved and resolved.name.startswith("darwin-") else resolved
        config_dir = config_dir_for(generation) if generation else home / ".config" / "brama" / "trust"
        runtime_dir = (
            home / ".stado" / "run" / f"brama-skarbiec-{generation.name}"
            if generation
            else home / ".stado" / "run" / "brama-skarbiec"
        )
        print("active release: absent; inspecting legacy current")
    print(f"config dir:  {config_dir}")
    print(f"runtime dir: {runtime_dir}")
    return config_dir
