#!/usr/bin/env python3
"""List the donated subscription rows the fleet's credential store holds.

Subscriptions are donated, not provisioned by hand: the reauth runner reads its
configuration from a `service_credentials` row and the donations land beside
it. The gateway's own `brama-sub-*` vault items are a projection of that set,
and when the projection is stale a fleet with working subscriptions looks empty
from the gateway side.

Read-only. Prints row ids, labels, providers and timestamps; the credential
column is never selected.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import urllib.error
import urllib.parse
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

value = (json.loads(read.stdout).get("fields") or {}).get("value")
if isinstance(value, str):
    value = json.loads(value)
metadata = value.get("metadata") if isinstance(value, dict) else None
if isinstance(metadata, str):
    metadata = json.loads(metadata)
if not isinstance(metadata, dict):
    print("the config carries no metadata object")
    raise SystemExit(1)

base = str(metadata.get("MR_SUPABASE_URL", "")).rstrip("/")
key = str(metadata.get("MR_SUPABASE_SERVICE_ROLE_KEY", ""))
if not base or not key:
    print("the config names no credential store")
    raise SystemExit(1)
print(f"store: {base}")


def rows(table: str, query: str) -> list | None:
    url = f"{base}/rest/v1/{table}?{query}"
    request = urllib.request.Request(url, headers={"apikey": key, "Authorization": f"Bearer {key}"})
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        print(f"  {table}: HTTP {error.code} {error.read()[:120].decode(errors='replace')}")
    except Exception as error:  # noqa: BLE001
        print(f"  {table}: {error}")
    return None


for table, select in (
    # Filtered and newest first, because the table keeps every revocation: an
    # unfiltered first page returned fifty rows, all of them revoked, and none of
    # the six that are actually live. The report read like a pool of dead keys.
    ("trade_agent_subscriptions", "status=eq.active&select=*&order=updated_at.desc"),
    ("trade_service_credentials", "select=id,updated_at"),
    ("service_credentials", "select=id,updated_at"),
):
    found = rows(table, select + "&limit=50")
    if found is None:
        continue
    print(f"{table}: {len(found)} row(s)")
    for row in found:
        if not isinstance(row, dict):
            continue
        shown = {
            column: row.get(column)
            # `key_label` is the column that carries the account a donated
            # credential belongs to, and `donor_id` who gave it. This report asked
            # for `label`, which the table does not have, so it printed every row
            # without ever naming the account -- and that is the one fact needed to
            # tell which paid subscription sits in which vault position.
            for column in ("id", "key_label", "donor_id", "provider", "status", "updated_at", "created_at")
            if row.get(column) is not None
        }
        print("  " + ", ".join(f"{column}={item}" for column, item in shown.items()))
