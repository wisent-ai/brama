#!/usr/bin/env python3
"""Enumerate every subscription in this host's pool, not a list written by hand.

The pool is not fixed: `put_donated_credential` mints `brama-sub-<agent>-...`
items as subscriptions are donated, so any report that names four items can
miss the ones that actually work. This walks the vault instead, and for each
item answers the three questions the dispatcher asks -- which provider, which
agents may use it, and whether `#value` yields a credential the adapter can
read.

Read-only, and prints no secret: the value is inspected only for which key
field it carries and how long that field is.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess

CLI = pathlib.Path.home() / ".stado/bin/skarbiec"
VAULT = pathlib.Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(pathlib.Path.home() / ".stado/skarbiec.vault.json"))
)
# The order the adapter tries, from SUPPORTED_KEY_FIELDS in providers/adapter.rs.
KEY_FIELDS = (
    ("key",), ("apiKey",), ("api_key",), ("access",), ("accessToken",), ("access_token",),
    ("token",), ("tokens", "access_token"), ("claudeAiOauth", "accessToken"),
)

environment = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": str(VAULT),
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}


def dig(document: object, path: tuple[str, ...]) -> object:
    cursor = document
    for part in path:
        if not isinstance(cursor, dict) or part not in cursor:
            return None
        cursor = cursor[part]
    return cursor


def peel(value: object) -> object:
    # A typed envelope is exactly {"type": ..., "value": ...}; peel until it is not.
    while isinstance(value, dict) and len(value) == 2 and "type" in value and "value" in value:
        value = value["value"]
    return value


listing = subprocess.run([str(CLI), "list"], capture_output=True, text=True, env=environment)
if listing.returncode:
    print(f"list failed: {listing.stderr.strip()[:200]}")
    raise SystemExit(1)

items = json.loads(listing.stdout)
if isinstance(items, dict):
    items = items.get("items") or []

pool = [
    item for item in items
    if isinstance(item, dict) and "brama-sub-" in str(item.get("id", "")) and item.get("deleted") is not True
]
print(f"subscriptions in the pool: {len(pool)}")

for item in sorted(pool, key=lambda entry: str(entry.get("id"))):
    item_id = str(item["id"])
    tags = [str(tag) for tag in (item.get("tags") or [])]
    agents = [tag.removeprefix("brama:agent:") for tag in tags if tag.startswith("brama:agent:")]
    provider = next((tag.removeprefix("brama:provider:") for tag in tags if tag.startswith("brama:provider:")), "?")

    read = subprocess.run([str(CLI), "get", item_id], capture_output=True, text=True, env=environment)
    if read.returncode:
        print(f"{item_id}: provider={provider} agents={','.join(agents) or 'none'} -> unreadable")
        continue
    document = json.loads(read.stdout)
    value = (document.get("fields") or {}).get("value")

    if isinstance(value, str):
        try:
            parsed = peel(json.loads(value))
        except json.JSONDecodeError:
            print(f"{item_id}: provider={provider} agents={','.join(agents) or 'none'} -> bare secret, {len(value)} chars")
            continue
    else:
        parsed = peel(value)

    carried = next(((path, dig(parsed, path)) for path in KEY_FIELDS if isinstance(dig(parsed, path), str)), None)
    if carried:
        path, secret = carried
        held = "string" if isinstance(value, str) else f"{type(value).__name__} (adapter reads strings)"
        print(f"{item_id}: provider={provider} agents={','.join(agents) or 'none'} -> {'.'.join(path)} present, {len(secret)} chars, stored as {held}")
    else:
        keys = ",".join(sorted(parsed)) if isinstance(parsed, dict) else type(parsed).__name__
        print(f"{item_id}: provider={provider} agents={','.join(agents) or 'none'} -> NO credential field; carries {keys}")
