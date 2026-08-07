#!/usr/bin/env python3
"""Say what the gateway would charge a `-best` request to, and what is missing.

`-best` resolves to a subscription route, so it needs an active subscription
credential owned by the calling agent. When there is none the answer is
`429 no active 'claude-code' credential for agent` — which says nothing about
whether the catalog is empty, the vault holds no such item, or a credential
exists on this host and was never donated to the gateway.

This prints all three, and never a secret: item ids, field names, and presence.
"""

import json
import os
import pathlib
import subprocess

PROVIDER = "claude-code"

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

router = settings.get("ENTITLEMENTS_ROUTER_BIN")
runtime_dir = pathlib.Path(settings.get("BRAMA_RUNTIME_DIR") or "/tmp/brama-skarbiec")
environment = {**os.environ, **settings}

print("=== subscription catalog the gateway was started with")
catalog = runtime_dir / "subscription-catalog.json"
if catalog.is_file():
    document = json.loads(catalog.read_text())
    items = document.get("items", []) if isinstance(document, dict) else []
    if not items:
        print("  empty")
    for entry in items:
        print(
            f"  {entry.get('id')}  provider={entry.get('provider')} "
            f"agent={entry.get('agent_id')} status={entry.get('status')}"
        )
else:
    print(f"  {catalog}: absent")

print("\n=== vault items for this provider")
listed = subprocess.run(
    [router, "list"], capture_output=True, text=True, check=False, env=environment
)
if listed.returncode:
    print(f"  cannot list: {listed.stderr.strip()}")
else:
    items = json.loads(listed.stdout)
    matching = [
        item.get("id")
        for item in items
        if isinstance(item, dict)
        and PROVIDER in str(item.get("id", ""))
        and not item.get("deleted", False)
    ]
    for identifier in sorted(matching) or ["  none"]:
        print(f"  {identifier}")

print("\n=== donation overlay the gateway reads")
donated = pathlib.Path(
    settings.get("BRAMA_DONATED_SUBSCRIPTIONS_FILE")
    or (runtime_dir / "donated-subscriptions.json")
)
if donated.is_file():
    document = json.loads(donated.read_text())
    entries = document.get("items", []) if isinstance(document, dict) else document
    for entry in entries or ["  none"]:
        print(f"  {entry}")
else:
    print(f"  {donated}: absent")

print("\n=== credential material present on this host")
for candidate in (
    home / ".claude" / ".credentials.json",
    home / "weles" / "var" / "claude-reauth",
    home / ".stado" / "brama-donated-claude-code.json",
):
    print(f"  {candidate}: {'present' if candidate.exists() else 'absent'}")

keychain = subprocess.run(
    ["/usr/bin/security", "find-generic-password", "-s", "Claude Code-credentials"],
    capture_output=True,
    text=True,
    check=False,
)
print(f"  login keychain 'Claude Code-credentials': {'present' if keychain.returncode == int() else 'absent'}")
