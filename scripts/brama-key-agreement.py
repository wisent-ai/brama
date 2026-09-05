#!/usr/bin/env python3
"""Say whether the vault names the key this installation will prove with.

`capability redemption denied` is what the broker returns when the token entry
for the agent carries a different public key than the installation's registry
does — or none at all, or an expired one. Three states, one message. This prints
the comparison: the key in the running installation's registry, the key in the
durable seed, and the key the vault the broker actually opens has on file.

Read-only.
"""

import base64
import hashlib
import json
import os
import pathlib
import subprocess

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

router = settings.get("ENTITLEMENTS_ROUTER_BIN")
running = (home / ".stado" / "services" / "brama" / "current").resolve()
architecture = running / "darwin-arm"
root = architecture if architecture.is_dir() else running
registry_path = root / "etc" / "brama-skarbiec" / "registry.json"

print(f"generation: {running.name}")
workload = next(
    iter(json.loads(registry_path.read_text()).get("workloads", {}).values()), {}
)
registry_key = workload.get("proof_key", "")
print(f"registry proof_key:      {registry_key}")

seed_file = pathlib.Path(
    settings.get("BRAMA_WORKLOAD_KEY_FILE")
    or (home / ".config" / "brama" / "brama-proof.key")
)
print(f"durable seed:            {'present' if seed_file.is_file() else 'absent'} ({seed_file})")

installed_seed = root / "etc" / "brama-skarbiec" / "brama-proof.key"
if seed_file.is_file() and installed_seed.is_file():
    same = seed_file.read_text().strip() == installed_seed.read_text().strip()
    print(f"installation uses it:    {same}")

listed = subprocess.run(
    [router, "tokens"],
    capture_output=True,
    text=True,
    check=False,
    env={**os.environ, **settings},
)
if listed.returncode:
    print(f"vault tokens unreadable: {listed.stderr.strip()}")
    raise SystemExit
document = json.loads(listed.stdout) if listed.stdout.strip() else {}
# The listing has been a dict of consumers, and a list of records, and a wrapper
# around either. Reading one shape and reporting ABSENT for the others is how a
# parser bug gets mistaken for a missing grant, so the shape is reported.
if isinstance(document, dict) and "tokens" in document:
    document = document["tokens"]
if isinstance(document, dict):
    entries = document
elif isinstance(document, list):
    entries = {
        record.get("consumer") or record.get("name"): record
        for record in document
        if isinstance(record, dict)
    }
else:
    entries = {}
print(f"vault listing shape:     {type(document).__name__}, entries: {', '.join(sorted(str(name) for name in entries))}")

for agent in workload.get("agent_ids", []):
    entry = entries.get(agent)
    if not isinstance(entry, dict):
        print(f"vault entry {agent}:  ABSENT")
        continue
    if entry.get("workload_bound") is True and "workload_public_key" not in entry:
        print(f"vault entry {agent}:  bound to a workload key the listing does not print")
        print(f"  expires_at:            {entry.get('expires_at')}")
        continue
    recorded = entry.get("workload_public_key")
    if not recorded:
        print(f"vault entry {agent}:  present but carries no workload key")
        continue
    body = "".join(
        line for line in recorded.splitlines() if "-----" not in line
    )
    raw = base64.b64decode(body)
    registry_raw = base64.b64decode(registry_key) if registry_key else b""
    agrees = raw.endswith(registry_raw) and bool(registry_raw)
    print(f"vault entry {agent}:  {'MATCHES the registry' if agrees else 'DIFFERENT KEY'}")
    print(f"  vault key digest:      {hashlib.sha256(raw).hexdigest()}")
    print(f"  expires_at:            {entry.get('expires_at')}")
