#!/usr/bin/env python3
"""Remove reauth rows' copies of the agent signing secret once proven stale.

Each reauth configuration row carried its own copy of
`WISENT_APP_AGENT_AUTH_SECRET`, and the copies had drifted: Brama answered 200 to
a read signed with `agent:<id>` and 401 to the same read signed with the row's
copy. The runners now sign with the agent's own item, so the copy has no reader
left -- and a stale secret with no reader is the trap the next person falls into.

A copy that still matches the agent item is left alone: it is not stale, and
removing a value nothing has contradicted is not this script's business.

Prints item names and verdicts. No secret is printed; comparison is by digest.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
ROWS = ("codex-reauth-config", "claude-reauth-config", "kimi-reauth-config")
KEY = "WISENT_APP_AGENT_AUTH_SECRET"
ENVIRONMENT = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": VAULT,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
}


def run(*args, stdin=NONE):
    return subprocess.run(
        args, capture_output=True, text=True, input=stdin, check=False, env=ENVIRONMENT
    )


def read(item):
    proc = run(str(SKARBIEC), "get", item)
    if proc.returncode != ZERO:
        return NONE
    return json.loads(proc.stdout)


def fingerprint(value):
    return hashlib.sha256(value.encode()).hexdigest()[: len("a" * 12)]


def main():
    for row in ROWS:
        document = read(row)
        if document is NONE:
            print(f"{row:<22} unreadable")
            continue
        value = document.get("fields", {}).get("value")
        if isinstance(value, str):
            try:
                value = json.loads(value)
            except ValueError:
                print(f"{row:<22} field value is not a document; left alone")
                continue
        if not isinstance(value, dict):
            print(f"{row:<22} no document; left alone")
            continue
        metadata = value.get("metadata")
        as_text = isinstance(metadata, str)
        if as_text:
            metadata = json.loads(metadata)
        if not isinstance(metadata, dict) or KEY not in metadata:
            print(f"{row:<22} carries no {KEY}")
            continue
        agent_id = metadata.get("WISENT_APP_AGENT_ID")
        agent = read(f"agent:{agent_id}") if agent_id else NONE
        current = (agent or {}).get("fields", {}).get("value")
        if not isinstance(current, str):
            print(f"{row:<22} agent:{agent_id} unreadable; left alone")
            continue
        if fingerprint(current) == fingerprint(metadata[KEY]):
            print(f"{row:<22} copy matches agent:{agent_id}; left alone")
            continue
        del metadata[KEY]
        value["metadata"] = json.dumps(metadata) if as_text else metadata
        document["fields"]["value"] = value
        written = run(str(SKARBIEC), "set-json", row, stdin=json.dumps(document))
        if written.returncode != ZERO:
            print(f"{row:<22} write refused: {written.stderr.strip().splitlines()[-1:]}")
            continue
        print(f"{row:<22} removed stale {KEY} (differed from agent:{agent_id})")
    return NONE


sys.exit(main())
