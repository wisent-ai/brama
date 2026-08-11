#!/usr/bin/env python3
"""Print the authenticated, non-billable Brama model catalog."""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import time
import urllib.request

base = os.environ["BRAMA_URL"].rstrip("/")
agent_id = os.environ.get("WISENT_APP_AGENT_ID", "wisent-app")
secret = os.environ["WISENT_APP_AGENT_AUTH_SECRET"]
token = os.environ.get("BRAMA_TOKEN", "")
timestamp = str(int(time.time()))
canonical = f"{agent_id}:{timestamp}:".encode()
headers = {
    "accept": "application/json",
    "x-agent-id": agent_id,
    "x-agent-timestamp": timestamp,
    "x-agent-body-sha256": "",
    "x-agent-signature": hmac.new(secret.encode(), canonical, hashlib.sha256).hexdigest(),
}
if token:
    headers["authorization"] = f"Bearer {token}"
request = urllib.request.Request(f"{base}/v1/models", headers=headers)
with urllib.request.urlopen(request, timeout=30) as response:
    payload = json.load(response)
models = payload.get("data", []) if isinstance(payload, dict) else []
available = [
    {
        "id": model.get("id"),
        "owned_by": model.get("owned_by"),
        "perf": model.get("perf"),
    }
    for model in models
    if model.get("available") is True
]
print(json.dumps({"available": available, "catalog_size": len(models)}, indent=2))
