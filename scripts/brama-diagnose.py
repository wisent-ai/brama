#!/usr/bin/env python3
"""Answer, on one host, why this gateway is or is not serving.

Every failure this host has had reads the same from outside - `/health` answers
and no route works, or nothing listens at all - while the cause was a different
file each time: a workload registry describing another installation, a router
without the verb the launcher calls, an operator route naming a deployment
instead of a provider, a policy granting a provider the broker never issued for,
a listener bound where no caller looks. Each was found by opening one more file
than the message named.

So this opens all of them, in the order the launcher does, and prints what each
says beside what it has to agree with:

  1. the units that start Brama, and which generation they lead to;
  2. every installed generation: completeness, router verbs, and whether its
     workload registry describes it on this host;
  3. the service env values that decide which of those is used;
  4. the policy's provider grants against the capabilities actually issued;
  5. every alias route against the providers a capability exists for;
  6. where the gateway is reachable, and by which scheme;
  7. the current boot attempt from the error log, and nothing older.

Read-only throughout.
"""

import hashlib
import json
import os
import pathlib
import plistlib
import shutil
import socket
import ssl
import subprocess
import time
import urllib.error
import urllib.request

SERVICE_LABEL = "com.wisent.always-on.brama"
CAPABILITY_VERB = "capability-issue"
REFUSAL = "unknown command"
BOOT_MARKER = "Starting server"
BEST_ALIAS = "-best"
REQUIRED_FILES = (
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
    "bin/provision-skarbiec-trust",
    "libexec/generate-skarbiec-config.mjs",
    "etc/brama-skarbiec/subscriptions.json",
)

home = pathlib.Path.home()
services = home / ".stado" / "services" / "brama"
env_file = home / ".config" / "brama" / "service.env"
error_log = home / ".stado" / "logs" / "brama-always-on.err"
uid = os.getuid()
gid = os.getgid()


def moment(epoch):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(epoch))


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


def architecture_root(generation):
    nested = generation / "darwin-arm"
    return nested if nested.is_dir() else generation


def router_answers(root):
    router = root / "bin" / "skarbiec-entitlements-router"
    if not router.is_file():
        return False
    probe = dict(os.environ)
    probe["SKARBIEC_VAULT_FILE"] = str(root / "no-such-vault.json")
    answered = subprocess.run(
        [str(router), CAPABILITY_VERB],
        capture_output=True,
        text=True,
        check=False,
        env=probe,
    )
    return REFUSAL not in (answered.stdout + answered.stderr)


def digest_of(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else "missing"


def registry_verdict(root):
    registry = root / "etc" / "brama-skarbiec" / "registry.json"
    if not registry.is_file():
        return ["no workload registry"]
    try:
        document = json.loads(registry.read_text())
    except ValueError as failure:
        return [f"registry unreadable: {failure}"]
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
    return wrong or ["describes this installation"]


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

print("\n=== installed generations")
for generation in sorted(services.iterdir(), key=lambda path: path.name):
    if generation.is_symlink() or not generation.is_dir():
        continue
    root = architecture_root(generation)
    if not (root / "bin").is_dir():
        continue
    marker = " <- current" if generation == resolved else ""
    missing = [name for name in REQUIRED_FILES if not (root / name).exists()]
    print(f"  {generation.name}{marker}  installed {moment(generation.stat().st_mtime)}")
    print(f"    files:    {'complete' if not missing else 'missing ' + ', '.join(missing)}")
    print(f"    router {CAPABILITY_VERB}: {router_answers(root)}")
    # A launcher without its executable bit is a unit that never starts and
    # never explains: launchd reports the failure to the system log, not to the
    # service's own, so the error stream stays exactly as it was on the last
    # successful boot and everything here reads as healthy.
    unrunnable = [
        name
        for name in ("bin/brama", "bin/skarbiec-entitlements-router", "bin/start-with-skarbiec")
        if (root / name).is_file() and not os.access(root / name, os.X_OK)
    ]
    if unrunnable:
        print(f"    NOT EXECUTABLE: {', '.join(unrunnable)}")
    for line in registry_verdict(root):
        print(f"    registry: {line}")

print("\n=== service env")
for name in sorted(settings):
    if "TOKEN" in name or "SECRET" in name or "KEY" in name or "PASSWORD" in name:
        print(f"  {name}=<redacted>")
    else:
        print(f"  {name}={settings[name]}")

config_dir = pathlib.Path(
    settings.get("BRAMA_SKARBIEC_CONFIG_DIR")
    or (architecture_root(resolved) / "etc" / "brama-skarbiec" if resolved else "")
)
runtime_dir = pathlib.Path(settings.get("BRAMA_RUNTIME_DIR") or "/tmp/brama-skarbiec")

print("\n=== policy grants against capabilities issued")
granted = set()
policy_path = config_dir / "policy.json"
if policy_path.is_file():
    policy = json.loads(policy_path.read_text())
    for rule in policy.get("roles", {}).get("brama-runtime", []):
        resource = rule.get("resource", "")
        if resource.startswith("provider:"):
            granted.add(resource.partition("provider:")[-len(["tail"])].split(":")[len([])])
    print(f"  policy.json written {moment(policy_path.stat().st_mtime)}")
    print(f"  granted providers ({len(granted)}): {', '.join(sorted(granted)) or 'none'}")
else:
    print(f"  {policy_path}: absent")

issued = {}
capabilities_path = runtime_dir / "provider-capabilities.json"
if capabilities_path.is_file():
    try:
        issued = json.loads(capabilities_path.read_text())
    except ValueError as failure:
        print(f"  provider-capabilities.json unreadable: {failure}")
    print(f"  provider-capabilities.json written {moment(capabilities_path.stat().st_mtime)}")
    print(f"  issued ({len(issued)}): {', '.join(sorted(issued)) or 'none'}")
    unissued = sorted(granted - set(issued))
    if unissued:
        print(f"  granted but never issued: {', '.join(unissued)}")
else:
    print(f"  {capabilities_path}: absent")

print("\n=== alias routes against issued capabilities")
routes_path = pathlib.Path(
    settings.get("BRAMA_INFERENCE_ROUTES_FILE")
    or (home / ".config" / "brama" / "inference-routes.json")
)
if routes_path.is_file():
    document = json.loads(routes_path.read_text())
    print(f"  {routes_path}")
    entries = dict(document.get("routes", {}))
    for alias, fallbacks in document.get("fallbacks", {}).items():
        for route in fallbacks:
            entries[f"{alias} (fallback)"] = route
    for alias, route in sorted(entries.items()):
        provider = route.split("/")[len([])]
        if alias.startswith(BEST_ALIAS):
            verdict = "exempt: a subscription pays for -best"
        elif "/" not in route:
            verdict = "REFUSED: names no provider"
        elif provider in issued:
            verdict = "ok"
        else:
            verdict = "REFUSED: no capability issued"
        print(f"    {alias} -> {route} [{verdict}]")
    deployments = [entry.get("name") for entry in document.get("deployments", [])]
    print(f"  deployments: {', '.join(name for name in deployments if name) or 'none'}")
else:
    print(f"  {routes_path}: absent")

print("\n=== reachability")
port = settings.get("PORT") or "8080"
announced = ""
if error_log.is_file():
    for line in error_log.read_text(errors="replace").splitlines():
        _, marker, remainder = line.partition("Starting brama server on ")
        if marker:
            announced = remainder.strip()
targets = [f"http://127.0.0.1:{port}/health"]
if announced:
    targets.append(f"http://{announced}/health")
tailscale = shutil.which("tailscale") or "/usr/local/bin/tailscale"
if pathlib.Path(tailscale).exists():
    status = subprocess.run(
        [tailscale, "status", "--json"], capture_output=True, text=True, check=False
    )
    if status.returncode == int():
        name = (json.loads(status.stdout).get("Self") or {}).get("DNSName", "").rstrip(".")
        if name:
            targets.append(f"https://{name}:8443/health")
    served = subprocess.run(
        [tailscale, "serve", "status"], capture_output=True, text=True, check=False
    )
    for line in (served.stdout + served.stderr).strip().splitlines():
        if "proxy" in line or "https://" in line:
            print(f"  serve: {line.strip()}")
relaxed = ssl.create_default_context()
relaxed.check_hostname = False
relaxed.verify_mode = ssl.CERT_NONE
for target in dict.fromkeys(targets):
    context = relaxed if target.startswith("https") else None
    try:
        with urllib.request.urlopen(target, context=context, timeout=float(len("ten"))) as answer:
            print(f"  {target} -> {answer.status}")
    except urllib.error.HTTPError as failure:
        print(f"  {target} -> HTTP {failure.code}")
    except (urllib.error.URLError, socket.timeout, ssl.SSLError, OSError) as failure:
        print(f"  {target} -> {failure}")

print("\n=== current boot attempt")
if error_log.is_file():
    text = error_log.read_text(errors="replace")
    segments = text.split(BOOT_MARKER)
    latest = BOOT_MARKER + segments.pop() if len(segments) > len([BOOT_MARKER]) else text
    print(latest.strip())
else:
    print(f"  {error_log}: absent")
# The slice above starts at the last time the gateway announced itself, which
# hides a start that never got that far: a launcher that dies while provisioning,
# registering or reading its own configuration prints before that line, not
# after. The raw tail is therefore not redundant with it.
#
# The per-provider refusals are dropped from this view. On a host whose vault
# backs two providers out of twenty-two they are twenty identical lines, and
# they push the launcher's own account of provisioning and registration - the
# part nothing else reports - out of any tail worth reading.
NOISE = ("capability issue failed for", "skipping subscription")
RAW_TAIL = len("a couple of dozen lines of the launcher's own account is what is worth")
print("\n=== last lines of the error stream, per-provider refusals dropped")
if error_log.is_file():
    raw = [
        line
        for line in error_log.read_text(errors="replace").splitlines()
        if not any(marker in line for marker in NOISE)
    ]
    for line in raw[-RAW_TAIL:] if len(raw) > RAW_TAIL else raw:
        print(f"  {line}")

# The broker and the launcher write to the unit's other stream, and a
# redemption refusal is reported there while the gateway's own log says only
# that a dependency was unavailable. Reading one and not the other is how "the
# credential is unavailable" stays a mystery.
TAIL_LINES = len("twenty lines is enough")
output_log = home / ".stado" / "logs" / "brama-always-on.out"
print("\n=== broker and launcher output")
if output_log.is_file():
    lines = output_log.read_text(errors="replace").splitlines()
    for line in lines[-TAIL_LINES:] if len(lines) > TAIL_LINES else lines:
        print(f"  {line}")
else:
    print(f"  {output_log}: absent")
