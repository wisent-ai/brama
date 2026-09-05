#!/usr/bin/env python3
"""Send refreshed subscription credentials to Brama instead of a dead URL.

The reauth orchestrators refresh a provider subscription and donate the result
to a model router. Their configuration still names
`https://model-router-...run.app`, a Cloud Run service that went away with the
GCP account, so every refreshed credential has nowhere to land -- which is why
the subscriptions read as dead while the accounts behind them are alive.

Brama implements that router's whole contract, and the registry already says
where Brama answers on this host, so the address is read from there rather than
restated here. The metadata map is merged, never replaced: it also carries the
agent identity, the HMAC secret and the expiry bookkeeping, and a bare write
would take them with it.

Idempotent. Prints addresses only; no secret is read out.
"""

import json
import os
import pathlib
import socket
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
ITEMS = ("codex-reauth-config", "claude-reauth-config", "kimi-reauth-config")
KEY = "MODEL_ROUTER_URL"
ENVIRONMENT = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": VAULT,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
}


def run(*args, stdin=NONE):
    return subprocess.run(
        args, capture_output=True, text=True, input=stdin, check=False, env=ENVIRONMENT
    )


def registry_document():
    proc = subprocess.run(
        [str(STADO), "registry", "pull"], capture_output=True, text=True, check=False
    )
    if proc.returncode == ZERO:
        return json.loads(proc.stdout)
    for candidate in (
        HOME / ".stado" / "local-storage" / "registry.json",
        HOME / ".stado" / "local-backup" / "registry.json",
    ):
        if candidate.is_file():
            return json.loads(candidate.read_text(encoding="utf-8"))
    raise SystemExit("no registry is readable from this host")


def this_target(document):
    node = socket.gethostname().lower()
    short = node.split(".")[ZERO]
    for entry in document.get("targets", []):
        names = [str(name).lower() for name in entry.get("hostnames", [])]
        names.append(str(entry.get("name", "")).lower())
        if any(name == node or name.split(".")[ZERO] == short for name in names if name):
            return entry.get("name")
    raise SystemExit(f"no registry target matches this machine ({node})")


def brama_endpoint():
    document = registry_document()
    here = this_target(document)
    service = document["service_directory"]["services"]["brama"]
    endpoint = service.get("endpoints", {}).get(here, {}).get("url")
    if not endpoint:
        raise SystemExit(f"the registry declares no brama endpoint on {here}")
    return endpoint.rstrip("/")


def main():
    wanted = brama_endpoint()
    print(f"brama      {wanted}")
    for item in ITEMS:
        proc = run(str(SKARBIEC), "get", item)
        if proc.returncode != ZERO:
            print(f"{item:<22} unreadable: {proc.stderr.strip().splitlines()[-1:]}")
            continue
        document = json.loads(proc.stdout)
        fields = document.get("fields", {})
        value = fields.get("value")
        if isinstance(value, str):
            try:
                value = json.loads(value)
            except ValueError:
                print(f"{item:<22} field value is not a document; left alone")
                continue
        if not isinstance(value, dict) or not isinstance(value.get("metadata"), (dict, str)):
            print(f"{item:<22} carries no metadata map; left alone")
            continue
        metadata = value["metadata"]
        as_text = isinstance(metadata, str)
        if as_text:
            metadata = json.loads(metadata)
        current = metadata.get(KEY, "(absent)")
        if current == wanted:
            print(f"{item:<22} settled at {wanted}")
            continue
        metadata[KEY] = wanted
        value["metadata"] = json.dumps(metadata) if as_text else metadata
        fields["value"] = value
        document["fields"] = fields
        written = run(str(SKARBIEC), "set-json", item, stdin=json.dumps(document))
        if written.returncode != ZERO:
            print(f"{item:<22} write refused: {written.stderr.strip().splitlines()[-1:]}")
            continue
        print(f"{item:<22} {current} -> {wanted}")
    return NONE


sys.exit(main())
