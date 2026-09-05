#!/usr/bin/env python3
"""List the subscriptions the model router actually serves for one agent.

The gateway's own pool is the set of `brama-sub-*` items in this host's vault,
and it is not the same set as the subscriptions the model router holds. When
the two are confused, a fleet with working subscriptions reads as a fleet with
none: the vault copy can be empty or stale while the router serves every one of
them.

The router is asked directly, the way the reauth runner asks it:
`GET {router}/v1/subscriptions/{agent}`.

Read-only. Prints subscription ids, providers, labels and status; the HMAC
secret and every token stay unprinted.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import urllib.error
import urllib.request

CLI = pathlib.Path.home() / ".stado/bin/skarbiec"
VAULT = pathlib.Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(pathlib.Path.home() / ".stado/skarbiec.vault.json"))
)
CONFIG_ITEM = os.environ.get("ROUTER_CONFIG_ITEM", "codex-reauth-config")

environment = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": str(VAULT),
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}

read = subprocess.run([str(CLI), "get", CONFIG_ITEM], capture_output=True, text=True, env=environment)
if read.returncode:
    print(f"{CONFIG_ITEM} unreadable: {read.stderr.strip()[:200]}")
    raise SystemExit(1)

document = json.loads(read.stdout)
value = (document.get("fields") or {}).get("value")
if isinstance(value, str):
    value = json.loads(value)
metadata = value.get("metadata") if isinstance(value, dict) else None
if isinstance(metadata, str):
    metadata = json.loads(metadata)
if not isinstance(metadata, dict):
    print(f"{CONFIG_ITEM} carries no metadata object")
    raise SystemExit(1)

router = str(metadata.get("MODEL_ROUTER_URL", "")).rstrip("/")
agent = str(metadata.get("WISENT_APP_AGENT_ID", ""))
print(f"router: {router or '(absent)'}")
print(f"agent:  {agent or '(absent)'}")
if not router or not agent:
    print("the config names no router or no agent; nothing to ask")
    raise SystemExit(1)

url = f"{router}/v1/subscriptions/{agent}"
try:
    with urllib.request.urlopen(url, timeout=20) as response:
        payload = json.loads(response.read())
except urllib.error.HTTPError as error:
    print(f"GET {url} -> HTTP {error.code}: {error.read()[:200].decode(errors='replace')}")
    raise SystemExit(1)
except Exception as error:  # noqa: BLE001 - the address itself may be unreachable
    print(f"GET {url} failed: {error}")
    raise SystemExit(1)

subscriptions = payload.get("subscriptions") if isinstance(payload, dict) else payload
if not isinstance(subscriptions, list):
    print(f"unexpected response shape: {type(subscriptions).__name__}")
    raise SystemExit(1)

print(f"subscriptions: {len(subscriptions)}")
for entry in subscriptions:
    if not isinstance(entry, dict):
        print(f"  {entry!r}")
        continue
    fields = {
        key: entry.get(key)
        for key in ("id", "provider", "label", "status", "state", "active", "revoked", "expires_at")
        if entry.get(key) is not None
    }
    print("  " + ", ".join(f"{key}={value}" for key, value in fields.items()))
