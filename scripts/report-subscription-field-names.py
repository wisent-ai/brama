#!/usr/bin/env python3
"""Print the field names each Brama subscription item carries.

The gateway redeems `#value` on these items and, when that fails, falls back to
a read of the same coordinate. Both paths reported the coordinate as absent
while the item itself existed with tags and recipients, so the open question is
narrow: which fields does the stored document actually have.

Read-only. Prints field names and whether each holds a non-empty string. No
field value is printed, and nothing is written.
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
ITEMS = [
    "provider:codex:brama-sub-wisent-app-codex-primary",
    "provider:codex:brama-sub-wisent-app-codex-secondary",
    "provider:claude-code:brama-sub-wisent-app-claude-primary",
    "provider:kimi:brama-sub-wisent-app-kimi-primary",
    "codex-reauth-config",
]
environment = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": str(VAULT),
    # Helpers run with a minimal PATH and the vault decrypts through gpg, so
    # without this the read fails as "spawn gpg" and looks like a missing item.
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}

# Which agents may use a subscription is carried as `brama:agent:<name>` tags on
# the item, and the router reports a missing tag and an unreadable credential
# with almost the same sentence. Printing both together is what tells the two
# apart.
listing = subprocess.run([str(CLI), "list"], capture_output=True, text=True, env=environment)
TAGS: dict[str, list[str]] = {}
if listing.returncode == 0:
    try:
        entries = json.loads(listing.stdout)
    except json.JSONDecodeError:
        entries = []
    if isinstance(entries, dict):
        entries = entries.get("items") or []
    for entry in entries:
        if isinstance(entry, dict) and isinstance(entry.get("id"), str):
            tags = entry.get("tags")
            TAGS[entry["id"]] = [str(tag) for tag in tags] if isinstance(tags, list) else []

for item in ITEMS:
    result = subprocess.run([str(CLI), "get", item], capture_output=True, text=True, env=environment)
    if result.returncode:
        print(f"{item}: unreadable: {result.stderr.strip()[:120]}")
        continue
    try:
        document = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        print(f"{item}: not JSON: {error}")
        continue
    fields = document.get("fields")
    if not isinstance(fields, dict):
        print(f"{item}: kind={document.get('kind')!r} carries no fields object")
        continue

    def describe(value: object) -> str:
        # "empty" and "not a string" are different answers and only one of them
        # is a defect: a reauth bundle is legitimately a JSON object, while an
        # empty string is the credential the gateway says it cannot find.
        if isinstance(value, str):
            return f"str({len(value)})" if value else "str(empty)"
        if isinstance(value, (dict, list)):
            return f"{type(value).__name__}({len(value)})"
        if value is None:
            return "null"
        return type(value).__name__

    shape = ", ".join(f"{name}={describe(value)}" for name, value in sorted(fields.items()))
    agents = [tag.removeprefix("brama:agent:") for tag in TAGS.get(item, []) if tag.startswith("brama:agent:")]
    print(f"{item}: kind={document.get('kind')!r} fields[{shape or 'none'}] agents[{','.join(agents) or 'none'}]")
