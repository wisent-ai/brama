#!/usr/bin/env python3
"""Print the shape of Brama's dedicated stado config.

The launcher reads the gateway's identity with `stado secrets get` under
`STADO_CONFIG=~/.config/stado/brama-service.json`. When that read goes to an
address nobody serves, the question is which key the backend actually consults,
and a config whose keys are unknown cannot answer it.

Read-only. Prints key paths and any value that is an address or a backend name;
never a token, key, or password.
"""

from __future__ import annotations

import json
import pathlib

SAFE_LEAVES = {"url", "backend", "namespace", "consumer", "item", "field", "kind", "mode"}
PATHS = [
    pathlib.Path.home() / ".config/stado/brama-service.json",
    pathlib.Path.home() / ".config/stado/config.json",
]


def walk(node: object, trail: tuple[str, ...] = ()) -> None:
    if isinstance(node, dict):
        for key, value in node.items():
            walk(value, trail + (str(key),))
        return
    if not trail:
        return
    leaf = trail[-1]
    dotted = ".".join(trail)
    if leaf in SAFE_LEAVES and isinstance(node, (str, bool, int)):
        print(f"  {dotted} = {node}")
    elif isinstance(node, (dict, list)):
        print(f"  {dotted} = <{type(node).__name__}>")
    else:
        print(f"  {dotted} = <redacted>")


for path in PATHS:
    print(f"=== {path} ===")
    try:
        document = json.loads(path.read_text())
    except FileNotFoundError:
        print("  absent")
        continue
    except json.JSONDecodeError as error:
        print(f"  unreadable: {error}")
        continue
    walk(document)
    print()
