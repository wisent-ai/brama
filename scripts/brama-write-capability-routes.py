#!/usr/bin/env python3
"""Write the table that maps capability resources to vault coordinates.

A capability names a purpose and a resource; only the issuing operator says
which vault entry that resource stands for, and `capability-routes.json` beside
the vault is the one place the two vocabularies meet. Without it the broker
refuses every issue, the gateway starts with no provider it may authenticate to,
and the first alias that needs one ends the process — with a message that names
none of this.

The mapping is not invented here. Every entry comes from an item the vault
already holds, and the field is chosen from that item's own field names by the
preference order below. An item whose fields name no credential is reported and
left out rather than guessed at.

Values are never read into this program's output: only item ids and field names
are printed.
"""

import json
import os
import pathlib
import shutil
import stat
import subprocess
import time

CREDENTIAL_FIELDS = ("api_key", "token", "access_token", "apiKey", "key", "secret", "value")
ROUTED_PREFIXES = ("provider:", "agent:")
OWNER_ONLY = stat.S_IRUSR | stat.S_IWUSR
INDENT = len("  ")

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

router = settings.get("ENTITLEMENTS_ROUTER_BIN")
vault = settings.get("SKARBIEC_VAULT_FILE")
if not router or not vault:
    raise SystemExit("service env must name ENTITLEMENTS_ROUTER_BIN and SKARBIEC_VAULT_FILE")

environment = dict(os.environ)
environment.update(settings)

listed = subprocess.run(
    [router, "list"], capture_output=True, text=True, check=False, env=environment
)
if listed.returncode:
    raise SystemExit(f"cannot list vault items: {listed.stderr.strip()}")
items = json.loads(listed.stdout)

# Whatever is already routed stays routed. The operator may have mapped a
# resource this scan cannot see - an item it cannot read, a resource whose
# credential lives under a field name not listed above - and replacing the file
# with only what one scan found would silently un-route it. Earlier copies are
# folded in for the same reason: a run that lost an entry is repaired by the
# next one instead of needing someone to notice.
routes = {}
existing = [pathlib.Path(vault).with_name("capability-routes.json")]
existing.extend(sorted(pathlib.Path(vault).parent.glob("capability-routes.json.before-*")))
for source in existing:
    if not source.is_file():
        continue
    try:
        recorded = json.loads(source.read_text())
    except ValueError:
        print(f"ignored unreadable {source.name}")
        continue
    for resource, entry in recorded.items():
        if isinstance(entry, dict) and entry.get("item") and entry.get("field"):
            routes.setdefault(resource, entry)
            print(f"kept {resource} from {source.name}")

unroutable = []
for item in items:
    if not isinstance(item, dict) or item.get("deleted", False):
        continue
    identifier = item.get("id")
    if not isinstance(identifier, str) or not identifier.startswith(ROUTED_PREFIXES):
        continue
    fetched = subprocess.run(
        [router, "get", identifier],
        capture_output=True,
        text=True,
        check=False,
        env=environment,
    )
    if fetched.returncode:
        unroutable.append(f"{identifier}: unreadable")
        continue
    try:
        fields = json.loads(fetched.stdout).get("fields", {})
    except ValueError:
        unroutable.append(f"{identifier}: answer is not an item")
        continue
    if not isinstance(fields, dict):
        unroutable.append(f"{identifier}: item carries no fields object")
        continue
    field = next((name for name in CREDENTIAL_FIELDS if name in fields), None)
    if field is None:
        unroutable.append(f"{identifier}: fields {sorted(fields)} name no credential")
        continue
    if routes.setdefault(identifier, {"item": identifier, "field": field}) == {
        "item": identifier,
        "field": field,
    }:
        print(f"{identifier} -> field {field}")

for line in unroutable:
    print(f"left out {line}")

if not routes:
    raise SystemExit("no vault item could be routed; nothing written")

target = pathlib.Path(vault).with_name("capability-routes.json")
if target.exists():
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    backup = target.with_name(f"{target.name}.before-{stamp}")
    shutil.copyfile(target, backup)
    print(f"previous copy: {backup}")
staging = target.with_name(f"{target.name}.staging")
staging.write_text(json.dumps(routes, indent=INDENT, sort_keys=True) + "\n")
staging.chmod(OWNER_ONLY)
staging.replace(target)
print(f"wrote {len(routes)} routes to {target}")
