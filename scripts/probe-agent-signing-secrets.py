#!/usr/bin/env python3
"""Say which stored secret Brama actually accepts for an agent's signature.

`agent authentication refused reason=auth: invalid signature` does not say which
of the copies of that secret is stale, and there are copies: the agent's own
vault item, and the metadata map each reauth row carries. Signing the same read
with each and reporting the status is the only way to tell them apart.

Prints status codes and item names. No secret and no signature is printed.
"""

import hashlib
import hmac
import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.error
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
ROUTER = os.environ.get("BRAMA_URL", "http://127.0.0.1:8080")
AGENT = "wisent-app"
BEARER_ITEM = f"{AGENT}-model-router"
CANDIDATES = (
    ("agent item", f"agent:{AGENT}", ("value",)),
    ("codex reauth row", "codex-reauth-config", ("value", "metadata", "WISENT_APP_AGENT_AUTH_SECRET")),
)
ENVIRONMENT = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": VAULT,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
}


def item(name):
    proc = subprocess.run(
        [str(SKARBIEC), "get", name], capture_output=True, text=True, check=False, env=ENVIRONMENT
    )
    if proc.returncode != ZERO:
        return NONE
    return json.loads(proc.stdout)


def dig(document, path):
    node = document.get("fields", {})
    for step in path:
        if isinstance(node, str):
            try:
                node = json.loads(node)
            except ValueError:
                return NONE
        if not isinstance(node, dict):
            return NONE
        node = node.get(step)
    if isinstance(node, str):
        return node
    return NONE


def probe(secret, bearer):
    stamp = str(int(time.time()))
    message = f"{AGENT}:{stamp}:".encode()
    signature = hmac.new(secret.encode(), message, hashlib.sha256).hexdigest()
    request = urllib.request.Request(
        f"{ROUTER}/v1/subscriptions/{AGENT}",
        headers={
            "x-agent-id": AGENT,
            "x-agent-timestamp": stamp,
            "x-agent-signature": signature,
            "authorization": f"Bearer {bearer}",
            "content-type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=len("aaaaaaaaaa")) as answer:
            return answer.status, len(answer.read())
    except urllib.error.HTTPError as error:
        return error.code, ZERO
    except OSError as error:
        return f"unreachable ({error})", ZERO


def main():
    bearer_document = item(BEARER_ITEM)
    bearer = dig(bearer_document or {}, ("token",)) or dig(bearer_document or {}, ("value",))
    print(f"router     {ROUTER}")
    print(f"bearer     {BEARER_ITEM} {'read' if bearer else 'unreadable'}")
    if not bearer:
        return len("x")
    for label, name, path in CANDIDATES:
        document = item(name)
        secret = dig(document or {}, path) if document else NONE
        if not secret:
            print(f"{label:<18} {name}: no secret at {'.'.join(path)}")
            continue
        status, size = probe(secret, bearer)
        print(f"{label:<18} {name}: HTTP {status}{f', {size} bytes' if size else ''}")
    return NONE


sys.exit(main())
