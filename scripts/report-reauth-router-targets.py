#!/usr/bin/env python3
"""Report where each reauth runner is told to donate a refreshed credential.

The runners refresh a subscription and hand the result to the model router.
Brama implements that router's whole contract -- list, donate, retire, and the
signed probe -- so a runner pointed at it works. Every one of them is instead
pointed at a Cloud Run service that went away with the GCP account, which is
why a refreshed credential has nowhere to land and why the subscriptions read
as dead while the accounts are alive.

This reads the rows the runners actually read, in the store they actually read
them from, rather than the Skarbiec copies of the same names.

Read-only. Prints row ids and router addresses; no key is printed.
"""

from __future__ import annotations

import json
import os
import pathlib
import urllib.error
import urllib.request

ENV_FILE = pathlib.Path(
    os.environ.get("WELES_WORKER_ENV_FILE", pathlib.Path.home() / ".config/weles/worker.env")
)
ROWS = ("codex-reauth-config", "claude-reauth-config", "kimi-reauth-config")


def worker_env() -> dict[str, str]:
    values: dict[str, str] = {}
    if not ENV_FILE.is_file():
        return values
    for line in ENV_FILE.read_text(errors="replace").splitlines():
        stripped = line.strip().removeprefix("export ").strip()
        name, separator, raw = stripped.partition("=")
        if separator and not stripped.startswith("#"):
            values[name.strip()] = raw.strip().strip('"').strip("'")
    return values


settings = {**worker_env(), **os.environ}
base = settings.get("SUPABASE_URL", "").rstrip("/")
key = settings.get("SUPABASE_SERVICE_ROLE_KEY", "")

print(f"env file: {ENV_FILE} ({'present' if ENV_FILE.is_file() else 'absent'})")
print(f"store:    {base or '(no SUPABASE_URL)'}")
if not base or not key:
    print("the worker environment names no credential store; the runners cannot read their config either")
    raise SystemExit(1)

for row in ROWS:
    url = f"{base}/rest/v1/service_credentials?id=eq.{row}&select=id,metadata"
    request = urllib.request.Request(url, headers={"apikey": key, "Authorization": f"Bearer {key}"})
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            payload = json.loads(response.read())
    except urllib.error.HTTPError as error:
        print(f"{row}: HTTP {error.code}")
        continue
    except Exception as error:  # noqa: BLE001
        print(f"{row}: {error}")
        continue
    if not payload:
        print(f"{row}: absent")
        continue
    metadata = payload[-len(payload)].get("metadata") or {}
    if isinstance(metadata, str):
        metadata = json.loads(metadata)
    router = metadata.get("MODEL_ROUTER_URL", "(absent)")
    print(f"{row}: MODEL_ROUTER_URL = {router}")
