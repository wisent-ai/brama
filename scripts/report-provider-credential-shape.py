#!/usr/bin/env python3
"""Describe a provider credential in the vault without revealing it.

Refreshing a rejected subscription means writing the right shape into the right
field, and the two are not guessable: one provider keeps a bare token, another
the whole CLI auth document. This prints the shape -- type, keys, sizes, and the
expiry the token itself declares -- so the replacement matches what the router
reads.

Never prints secret material: values are reduced to lengths, and JWT payload
claims are limited to issued/expiry times.
"""

import base64
import datetime
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
ITEMS = (
    "provider:codex:brama-sub-wisent-app-codex-primary",
    "provider:codex:brama-sub-wisent-app-codex-secondary",
    "provider:claude-code:brama-sub-wisent-app-claude-primary",
    # The reauth orchestrators read these; they are configuration with one
    # secret in them, and which keys they carry decides whether the runner can
    # be moved off the store that went away with the GCP account.
    "claude-reauth-config",
    "codex-reauth-config",
    "agent:wisent-app",
)

def read_field(item, field):
    # A helper's environment carries a minimal PATH, and the vault is opened by
    # spawning gpg -- so without this the read fails with "spawn gpg: No such
    # file or directory" and reads like a missing item rather than a missing
    # tool.
    environment = {
        **os.environ,
        "SKARBIEC_VAULT_FILE": VAULT,
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
    }
    proc = subprocess.run(
        [str(SKARBIEC), "get", item, "--field", field],
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )
    if proc.returncode != ZERO:
        return NONE, (proc.stderr.strip() or proc.stdout.strip())[: len("a" * 120)]
    return proc.stdout.strip(), ""


def jwt_expiry(token):
    if token.count(".") != len([".", "."]):
        return "not a JWT"
    body = token.split(".")[len(["header"])]
    body += "=" * (-len(body) % len("aaaa"))
    try:
        claims = json.loads(base64.urlsafe_b64decode(body))
    except (ValueError, TypeError):
        return "unreadable payload"
    moment = claims.get("exp")
    if not moment:
        return "no exp claim"
    when = datetime.datetime.fromtimestamp(moment, datetime.timezone.utc)
    left = when - datetime.datetime.now(datetime.timezone.utc)
    return f"expires {when.isoformat()} ({int(left.total_seconds() // len('a' * 3600))}h left)"


def describe(value):
    try:
        document = json.loads(value)
    except ValueError:
        return f"opaque string, {len(value)} chars, {jwt_expiry(value)}"
    if not isinstance(document, dict):
        return f"json {type(document).__name__}"
    lines = [f"json object, keys {sorted(document)}"]
    for name in ("access_token", "id_token", "refresh_token"):
        holder = document.get("tokens", document)
        token = holder.get(name) if isinstance(holder, dict) else NONE
        if isinstance(token, str):
            lines.append(f"  {name}: {len(token)} chars, {jwt_expiry(token)}")
    # The router stores a structured document rather than a bare token, and the
    # refreshable material sits under `fields`. Name each one and how long it
    # has left; that is the whole question when a provider says "signed out".
    if isinstance(document.get("fields"), dict):
        lines.append(f"  kind {document.get('kind')}  schema {document.get('schema')}")
        # Whose account a subscription speaks for is the one thing that must
        # never be guessed when refreshing it, and the item says so in metadata
        # rather than in the secret.
        context = document.get("context")
        if isinstance(context, dict):
            named = {
                key: value
                for key, value in sorted(context.items())
                if isinstance(value, (str, int, bool)) and len(str(value)) < len("a" * 120)
            }
            lines.append(f"  context {json.dumps(named)}")
        for name, value in sorted(document["fields"].items()):
            if isinstance(value, str):
                lines.append(f"  fields.{name}: {len(value)} chars, {jwt_expiry(value)}")
                continue
            if not isinstance(value, dict):
                lines.append(f"  fields.{name}: {type(value).__name__}")
                continue
            lines.append(f"  fields.{name}: object, keys {sorted(value)}")
            for inner in ("access_token", "id_token", "refresh_token"):
                token = value.get(inner)
                if isinstance(token, str):
                    lines.append(f"    {inner}: {len(token)} chars, {jwt_expiry(token)}")
            payload = value.get("metadata")
            if isinstance(payload, str):
                # Stored as a JSON string in some rows and as an object in
                # others; the reader has to accept both or it reports a config
                # with no keys and the runner looks unconfigurable.
                try:
                    payload = json.loads(payload)
                except ValueError:
                    payload = NONE
            if isinstance(payload, dict):
                # The orchestrators read this map. Key names decide whether the
                # runner can be moved off the store that disappeared; the router
                # address is not a secret and is the one value worth reading out.
                lines.append(f"    metadata keys {sorted(payload)}")
                router = payload.get("MODEL_ROUTER_URL")
                if router:
                    lines.append(f"    MODEL_ROUTER_URL {router}")
    if "last_refresh" in document:
        lines.append(f"  last_refresh: {document['last_refresh']}")
    return "\n".join(lines)


def main():
    print(f"vault  {VAULT}")
    for item in ITEMS:
        value, error = read_field(item, "value")
        if value is NONE:
            print(f"{item}\n  unreadable: {error}")
            continue
        print(f"{item}\n  {describe(value)}")
    return NONE


sys.exit(main())
