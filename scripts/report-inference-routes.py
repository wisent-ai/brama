#!/usr/bin/env python3
"""Report where the gateway's own model routes point, and whether they answer.

`wisent-backend/chat/primary` is not a provider model: it is a route in this
host's inference routes file, and when the endpoint behind it stops answering
the caller sees only "provider request failed". That message is the same one a
missing credential produces, so the route table and a live probe of each
endpoint are what separate them.

Read-only. Prints route ids and endpoint addresses, never a token; each
endpoint is probed with a HEAD-equivalent request that carries no credential.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import urllib.parse

path = pathlib.Path(
    os.environ.get("BRAMA_INFERENCE_ROUTES_FILE", pathlib.Path.home() / ".stado/inference/routes.json")
)

print(f"file: {path}")
if not path.is_file():
    print("state: absent")
    raise SystemExit(0)

try:
    document = json.loads(path.read_text())
except json.JSONDecodeError as error:
    print(f"unreadable: {error}")
    raise SystemExit(1)

routes = document.get("routes") if isinstance(document, dict) else document
if isinstance(routes, dict):
    routes = [{"id": key, **value} if isinstance(value, dict) else {"id": key} for key, value in routes.items()]
if not isinstance(routes, list):
    print(f"unexpected shape: {type(routes).__name__}")
    raise SystemExit(1)

seen: set[str] = set()
for route in routes:
    if not isinstance(route, dict):
        continue
    name = route.get("id") or route.get("name") or route.get("model") or "?"
    endpoint = route.get("endpoint") or route.get("base_url") or route.get("url") or ""
    if not endpoint:
        # The key names differ between generations of this file, and guessing
        # them is how a route with a perfectly good address reads as empty.
        def safe(node: object) -> object:
            if isinstance(node, dict):
                return {
                    key: ("<redacted>" if any(mark in key.lower() for mark in ("token", "key", "secret", "password")) else safe(value))
                    for key, value in node.items()
                }
            if isinstance(node, list):
                return [safe(entry) for entry in node]
            return node

        print(f"{name}: {json.dumps(safe(route))[:300]}")
        continue
    print(f"{name}: {endpoint}")
    if endpoint in seen:
        continue
    seen.add(endpoint)
    parsed = urllib.parse.urlsplit(endpoint)
    probe = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, "/", "", ""))
    result = subprocess.run(
        ["/usr/bin/curl", "-s", "-m", "6", "-o", "/dev/null", "-w", "%{http_code}", probe],
        capture_output=True,
        text=True,
    )
    code = result.stdout.strip() or "000"
    print(f"    probe {probe} -> HTTP {code}{'  (nothing listening)' if code == '000' else ''}")
