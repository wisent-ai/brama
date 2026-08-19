#!/usr/bin/env python3
"""Write the capability routes this host's own vault already determines.

A resource names a purpose; the routes file says which vault coordinate that
purpose stands for. Nothing else on this host writes it, so a resource the table
does not mention is refused at `capability-issue` -- "no capability route maps
<resource> to a vault field" -- and the gateway's read grant has nothing to
resolve either, which reaches a caller as a credential that is merely
"unavailable".

The gateway's launcher runs this at every start, so a subscription banked after
the last start becomes spendable at the next one instead of waiting for somebody
to remember a helper.

The design keeps this decision away from the workload for a good reason: a
gateway that picked its own mapping would be picking which credential its
purpose stands for. This is not that. It runs on the operator's side, and it
records only mappings the host's contents already fix, with no room to choose:

  * the item is the resource -- the launcher builds resources FROM item ids,
    so the two are the same string, not a match this program invents,
  * the field is taken only when the item carries exactly ONE. Two or more and
    there is a real choice to make, so it refuses and names them for a human.

Additive: every entry an existing table carries stays exactly as written, so an
operator's own mapping always wins and a resource deliberately pointed
elsewhere is never repointed. Only resources the table does not mention at all
are added, the previous table is kept beside the new one, and the run reports
where. Adding a coordinate widens nothing: redemption is authorised by the
workload key the vault registers and the recipients the item carries, never by
this table.

Prints item ids and field names, never a value.
"""
from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
BUNDLE = HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm"
ROUTER = Path(os.environ.get("ENTITLEMENTS_ROUTER_BIN", str(BUNDLE / "bin" / "skarbiec-entitlements-router")))
SERVICE_ENV = Path(os.environ.get("BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")))
RESOURCE_PREFIXES = ("provider:", "provider-", "agent:", "agent-")
GPG_DIRECTORIES = ("/opt/homebrew/bin", "/usr/local/bin", "/usr/bin")


def vault_path() -> str:
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
    search = [*GPG_DIRECTORIES, *environment.get("PATH", "").split(os.pathsep)]
    environment["PATH"] = os.pathsep.join(part for part in search if part)
    return subprocess.run([str(ROUTER), *arguments], capture_output=True, text=True, env=environment)


def first_line(text: str) -> str:
    head, _separator, _rest = text.partition("\n")
    return head


VAULT = vault_path()
# Path("") is Path("."), which is truthy and exists, so the empty environment
# value has to be rejected as a string before it ever becomes a path.
_configured_routes = os.environ.get("SKARBIEC_CAPABILITY_ROUTES_FILE", "").strip()
ROUTES = Path(_configured_routes) if _configured_routes else Path(VAULT).with_name("capability-routes.json")
print("vault:", VAULT)
print("routes:", ROUTES)

if not ROUTER.exists():
    raise SystemExit("entitlements router is absent; nothing can be provisioned")
# Additive, never destructive. An existing table is an operator's decision and
# every entry in it stays exactly as written; only resources it does not
# mention at all can be added, and only where the host's own contents leave no
# choice about what they mean. A resource the operator deliberately left out
# with a different mapping is therefore never touched.
existing = {}
if ROUTES.exists():
    try:
        loaded = json.loads(ROUTES.read_text(errors="replace"))
    except ValueError as error:
        raise SystemExit(f"the routes table does not parse; refusing to touch it: {error}")
    if not isinstance(loaded, dict):
        raise SystemExit("the routes table is not an object of resources; refusing to touch it")
    existing = loaded
    print("existing routes:", len(existing))

listed = router("list")
if listed.returncode:
    raise SystemExit(f"listing the vault failed: {first_line(listed.stderr.strip()) or 'no detail'}")

resources = sorted(
    entry["id"]
    for entry in json.loads(listed.stdout)
    if isinstance(entry, dict)
    and not entry.get("deleted", False)
    and isinstance(entry.get("id"), str)
    and entry["id"].startswith(RESOURCE_PREFIXES)
)

routes = {}
skipped = []
for item in resources:
    if item in existing:
        continue
    got = router("get", item)
    if got.returncode:
        skipped.append((item, first_line(got.stderr.strip()) or "no detail"))
        continue
    fields = json.loads(got.stdout).get("fields")
    if not isinstance(fields, dict) or not fields:
        skipped.append((item, "no fields"))
        continue
    names = sorted(fields)
    try:
        (single,) = names
    except ValueError:
        skipped.append((item, f"operator must choose among {names}"))
        continue
    routes[item] = {"item": item, "field": single}
    print("adding:", item, "->", single)

for item, reason in skipped:
    print("skipped:", item, "--", reason)

if not routes:
    print("status: nothing to add; every resource this host can resolve is already mapped")
    raise SystemExit()

merged = dict(existing)
merged.update(routes)
staging = ROUTES.with_name(ROUTES.name + ".staging")
staging.write_text(json.dumps(merged, indent=None, sort_keys=True) + "\n", encoding="utf-8")
os.chmod(staging, stat.S_IRUSR | stat.S_IWUSR)
if ROUTES.exists():
    shutil.copy2(ROUTES, ROUTES.with_name(ROUTES.name + ".before-add"))
    print("kept the table as it was at:", ROUTES.with_name(ROUTES.name + ".before-add"))
os.rename(staging, ROUTES)
print("status: added", len(routes), "routes to", len(existing), "already there")
print("undo: mv", ROUTES.with_name(ROUTES.name + ".before-add"), ROUTES)
