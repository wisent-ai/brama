#!/usr/bin/env python3
"""Report why the broker accepts or refuses a Brama installation's workload.

`capability redemption denied: peer mismatch` is the broker saying the process
asking to redeem is not the workload `registry.json` describes. The registry
pins four things - executable path, executable digest, uid and gid - and the
message names none of them, so a diagnosis has meant guessing one and
restarting.

This prints, for every installed generation, what its registry pins beside
what the host actually has; then the two runtime values that decide which
registry is even consulted - `BRAMA_BIN` and `BRAMA_SKARBIEC_CONFIG_DIR` -
and the binary the live listener is running. A mismatch between a generation
that provisioned itself correctly and a service env still pointing at another
generation is invisible in the broker's message and obvious here.

Read-only: it starts nothing, stops nothing, writes nothing.
"""

import hashlib
import json
import os
import pathlib
import subprocess

home = pathlib.Path.home()
services = home / ".stado" / "services" / "brama"
current = services / "current"
resolved_current = current.resolve() if current.exists() else None
uid = os.getuid()
gid = os.getgid()


def digest(path):
    if not path.is_file():
        return "missing"
    return hashlib.sha256(path.read_bytes()).hexdigest()


def report_generation(generation):
    architecture = generation / "darwin-arm"
    if not architecture.is_dir():
        architecture = generation
    registry_path = architecture / "etc" / "brama-skarbiec" / "registry.json"
    binary = architecture / "bin" / "brama"
    marker = " <- current" if generation == resolved_current else ""
    print(f"\n=== {generation.name}{marker}")
    if not registry_path.is_file():
        print("  registry: MISSING - this installation has no workload identity")
        return
    document = json.loads(registry_path.read_text())
    workload = next(iter(document.get("workloads", {}).values()), {})
    observed = {
        "uid": uid,
        "gid": gid,
        "executable_path": str(binary),
        "executable_sha256": digest(binary),
    }
    for field, actual in observed.items():
        pinned = workload.get(field, "unset")
        verdict = "MATCH" if str(pinned) == str(actual) else "MISMATCH"
        print(f"  {field} {verdict} pinned={pinned} actual={actual}")
    requirement = workload.get("macos_code_signing_requirement", "none recorded")
    print(f"  code_requirement {requirement}")


if services.is_dir():
    for generation in sorted(services.iterdir()):
        if generation.is_dir() and not generation.is_symlink():
            report_generation(generation)

print("\n=== service env")
env_file = home / ".config" / "brama" / "service.env"
interesting = ("BRAMA_BIN", "BRAMA_SKARBIEC_CONFIG_DIR", "ENTITLEMENTS_ROUTER_BIN")
if env_file.is_file():
    for line in env_file.read_text().splitlines():
        if line.startswith(interesting):
            print(f"  {line}")
else:
    print(f"  {env_file}: absent")

print("\n=== live listener")
listeners = subprocess.run(
    ["/usr/sbin/lsof", "-nP", "-iTCP:8080", "-sTCP:LISTEN", "-Fpn"],
    capture_output=True,
    text=True,
    check=False,
)
pids = [line.lstrip("p") for line in listeners.stdout.splitlines() if line.startswith("p")]
if not pids:
    print("  nothing is listening on the service port")
for pid in dict.fromkeys(pids):
    running = subprocess.run(
        ["/bin/ps", "-o", "comm=", "-p", pid],
        capture_output=True,
        text=True,
        check=False,
    )
    executable = running.stdout.strip()
    print(f"  pid={pid} executable={executable}")
    candidate = pathlib.Path(executable)
    if candidate.is_file():
        print(f"  running_sha256={digest(candidate)}")
