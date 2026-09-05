#!/usr/bin/env python3
"""Point Brama's service config at the Skarbiec address this host declares.

The launcher reads the gateway's own GPG identity through a dedicated stado
config, `~/.config/stado/brama-service.json`, and that file carried a Skarbiec
address of its own. When the daemon moved onto the port its placement profile,
its service-directory endpoint and the ssh permitopen list had all named for
months, this one file still pointed at the binary's old default, so the gateway
could no longer read its identity and refused to start.

The address is copied from the host's main stado config rather than written
here, so the two cannot drift again. Idempotent, and prints addresses only --
this file never reads a secret.
"""

from __future__ import annotations
import json
import os
import pathlib
import sys

KEY = ("secrets", "skarbiec", "url")
HOME = pathlib.Path.home()
MAIN = HOME / ".config/stado/config.json"
SERVICE = HOME / ".config/stado/brama-service.json"


def read(path: pathlib.Path) -> dict:
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        print(f"absent: {path}")
        raise SystemExit(1)
    except json.JSONDecodeError as error:
        print(f"unreadable: {path}: {error}")
        raise SystemExit(1)


def get(document: dict, key: tuple[str, ...]) -> str | None:
    cursor: object = document
    for part in key:
        if not isinstance(cursor, dict) or part not in cursor:
            return None
        cursor = cursor[part]
    return cursor if isinstance(cursor, str) else None


def put(document: dict, key: tuple[str, ...], value: str) -> None:
    cursor = document
    for part in key[:-1]:
        nxt = cursor.get(part)
        if not isinstance(nxt, dict):
            nxt = {}
            cursor[part] = nxt
        cursor = nxt
    cursor[key[-1]] = value


main = read(MAIN)
service = read(SERVICE)

declared = get(main, KEY)
if not declared:
    print(f"the host config declares no {'.'.join(KEY)}; nothing to copy")
    sys.exit(1)

current = get(service, KEY)
print(f"host declares:      {declared}")
print(f"service config:     {current or '(unset)'}")

if current != declared:
    put(service, KEY, declared)
    SERVICE.write_text(json.dumps(service, indent=2) + "\n")
    print(f"aligned {SERVICE}")

# The environment file wins over the config: the launcher sources it, so a
# stale WC_SKARBIEC_URL here silently overrides everything above. This is the
# record that actually decided the address, and leaving it behind is how the
# same repair looks applied and changes nothing.
ENV_FILE = pathlib.Path(
    os.environ.get("BRAMA_SERVICE_ENV_FILE", str(HOME / ".config/brama/service.env"))
)
ENV_KEY = "WC_SKARBIEC_URL"

if not ENV_FILE.is_file():
    print(f"service env:        absent at {ENV_FILE}")
    sys.exit(0)

lines = ENV_FILE.read_text().splitlines()
pinned = None
for index, line in enumerate(lines):
    stripped = line.strip().removeprefix("export ").strip()
    name, separator, raw = stripped.partition("=")
    if separator and name.strip() == ENV_KEY:
        pinned = raw.strip().strip('"').strip("'")
        if pinned != declared:
            lines[index] = f"{ENV_KEY}={declared}"
        break

print(f"service env:        {pinned or '(unset)'}")
if pinned == declared:
    print("already aligned")
    sys.exit(0)
if pinned is None:
    lines.append(f"{ENV_KEY}={declared}")
ENV_FILE.write_text("\n".join(lines) + "\n")
print(f"aligned {ENV_FILE} to {declared}")
