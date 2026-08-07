#!/usr/bin/env python3
"""Qualify every operator route with the provider that actually serves it.

`inference-routes.json` overrides the launch aliases, and the server requires
each route except `-best` to name a provider it holds a capability for. A bare
route like `chat-primary` names a deployment, not a provider, so
`provider_id_from_route` yields nothing and the gateway refuses to start with a
message naming one alias at a time.

A bare route is repaired only when a deployment in the same file carries that
name - that is the evidence the value is a local deployment - and it becomes
`local-openai/<deployment>`, which is how the launcher writes it. Anything
else is left alone and reported, because guessing a provider for a route
nobody can identify is how the file got into this state.

The previous file is kept beside the new one.
"""

import json
import pathlib
import shutil
import time

LOCAL_PROVIDER = "local-openai"

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

routes_path = pathlib.Path(
    settings.get("BRAMA_INFERENCE_ROUTES_FILE")
    or (home / ".config" / "brama" / "inference-routes.json")
)
if not routes_path.is_file():
    raise SystemExit(f"no operator route file at {routes_path}")

document = json.loads(routes_path.read_text())
deployments = {entry.get("name") for entry in document.get("deployments", [])}
print(f"routes: {routes_path}")
print(f"deployments: {', '.join(sorted(name for name in deployments if name)) or 'none'}")

changed = []
unresolved = []


def qualify(route):
    if "/" in route:
        return route
    if route in deployments:
        return f"{LOCAL_PROVIDER}/{route}"
    unresolved.append(route)
    return route


routes = document.get("routes", {})
for alias, route in list(routes.items()):
    qualified = qualify(route)
    if qualified != route:
        routes[alias] = qualified
        changed.append(f"{alias}: {route} -> {qualified}")

fallbacks = document.get("fallbacks", {})
for alias, entries in list(fallbacks.items()):
    rewritten = [qualify(route) for route in entries]
    if rewritten != entries:
        fallbacks[alias] = rewritten
        changed.append(f"{alias} (fallback): {entries} -> {rewritten}")

for line in changed:
    print(f"qualified {line}")
for route in sorted(set(unresolved)):
    print(f"unresolved {route}: names no deployment, left unchanged")

if not changed:
    print("nothing to repair")
    raise SystemExit

stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
backup = routes_path.with_name(f"{routes_path.name}.before-qualify-{stamp}")
shutil.copyfile(routes_path, backup)
temporary = routes_path.with_name(f"{routes_path.name}.stado-repair")
temporary.write_text(json.dumps(document, indent=len("  "), sort_keys=True) + "\n")
temporary.chmod(routes_path.stat().st_mode)
temporary.replace(routes_path)
print(f"previous copy: {backup}")
