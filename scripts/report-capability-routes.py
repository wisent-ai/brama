#!/usr/bin/env python3
"""Report which capability routes this host's vault would support.

A resource names a purpose. Only the issuing operator says which vault entry
that purpose stands for, and that mapping lives in one file beside the vault.
Without it every `capability-issue` is refused, the gateway starts with no
provider it may authenticate to, and it dies on the first alias that needed
one -- an error naming none of this.

Writing that mapping by guessing is the thing the design forbids: a wrong
guess hands out a credential the operator never authorised for that purpose.
So this guesses nothing. It reports what is already true on the host:

  * the item ids the vault holds for provider and agent resources,
  * the FIELD NAMES each item carries, never a value,
  * whether the field is unambiguous -- exactly one, so no choice is being
    made -- or several, in which case the operator picks and this says so.

Read-only. It writes no file, mints nothing, and prints no secret.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
BUNDLE = HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm"
ROUTER = Path(os.environ.get("ENTITLEMENTS_ROUTER_BIN", str(BUNDLE / "bin" / "skarbiec-entitlements-router")))
SERVICE_ENV = Path(os.environ.get("BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")))
RESOURCE_PREFIXES = ("provider:", "provider-", "agent:", "agent-")
GPG_DIRECTORIES = ("/opt/homebrew/bin", "/usr/local/bin", "/usr/bin")


def vault_path() -> str:
    """The vault the launcher itself would use, read from the same file."""
    configured = os.environ.get("SKARBIEC_VAULT_FILE", "")
    if configured:
        return configured
    if SERVICE_ENV.is_file():
        for line in SERVICE_ENV.read_text(errors="replace").splitlines():
            name, sep, value = line.partition("=")
            if sep and name.strip() == "SKARBIEC_VAULT_FILE":
                return value.strip().strip('"').strip("'")
    return str(HOME / ".stado" / "skarbiec.vault.json")


def router(*arguments: str) -> subprocess.CompletedProcess:
    environment = dict(os.environ)
    environment["SKARBIEC_VAULT_FILE"] = VAULT
    # A helper is run with a minimal environment, and the vault is opened by
    # spawning gpg. Without a PATH that reaches it every item reads as
    # unreadable, which would libel the host for a defect in this script.
    search = [*GPG_DIRECTORIES, *environment.get("PATH", "").split(os.pathsep)]
    environment["PATH"] = os.pathsep.join(part for part in search if part)
    return subprocess.run(
        [str(ROUTER), *arguments],
        capture_output=True,
        text=True,
        env=environment,
    )


def first_line(text: str) -> str:
    head, _separator, _rest = text.partition("\n")
    return head


VAULT = vault_path()
print("router:", ROUTER, "exists:", ROUTER.exists())
print("vault:", VAULT, "exists:", Path(VAULT).exists())

# The file's absence and the file's emptiness produce the same refusal from the
# authority and are entirely different repairs, so say which one this host has.
_configured_routes = os.environ.get("SKARBIEC_CAPABILITY_ROUTES_FILE", "").strip()
ROUTES = Path(_configured_routes) if _configured_routes else Path(VAULT).with_name("capability-routes.json")
print("routes:", ROUTES, "exists:", ROUTES.exists())
if ROUTES.exists():
    try:
        existing = json.loads(ROUTES.read_text(errors="replace"))
    except ValueError as error:
        print("routes file does not parse:", error)
        existing = {}
    if isinstance(existing, dict):
        print("routes mapped:", len(existing))
        for resource in sorted(existing):
            print("  mapped:", resource, "->", existing[resource])
    else:
        print("routes file is not an object of resources")
if not ROUTER.exists():
    raise SystemExit("entitlements router is absent; nothing can be reported")

listed = router("list")
if listed.returncode:
    raise SystemExit(f"listing the vault failed: {first_line(listed.stderr.strip()) or 'no detail'}")

items = json.loads(listed.stdout)
resources = sorted(
    entry["id"]
    for entry in items
    if isinstance(entry, dict)
    and not entry.get("deleted", False)
    and isinstance(entry.get("id"), str)
    and entry["id"].startswith(RESOURCE_PREFIXES)
)
print("resource items:", len(resources))

derivable = {}
ambiguous = {}
unreadable = []
for item in resources:
    got = router("get", item)
    if got.returncode:
        unreadable.append(item)
        print("unreadable:", item, "--", first_line(got.stderr.strip()) or "no detail")
        continue
    fields = json.loads(got.stdout).get("fields")
    if not isinstance(fields, dict) or not fields:
        unreadable.append(item)
        print("no fields:", item)
        continue
    names = sorted(fields)
    try:
        (single,) = names
    except ValueError:
        ambiguous[item] = names
        print("ambiguous:", item, "fields:", names)
        continue
    derivable[item] = single
    print("derivable:", item, "field:", single)

print()
print("derivable:", len(derivable), "ambiguous:", len(ambiguous), "unreadable:", len(unreadable))
print()
print("routes that follow from the host's own contents, for the operator to accept or edit:")
print(json.dumps({item: {"item": item, "field": field} for item, field in derivable.items()}, sort_keys=True))
