#!/usr/bin/env python3
"""Report where the gateway's own model routes point.

`wisent-backend/chat/primary` is not a provider model: it is a route in this
host's inference routes file, and when the endpoint behind it stops answering
the caller sees only "provider request failed". That message is the same one a
missing credential produces, so this report shows both the ordered route table
and the resolved endpoint.

Read-only. Prints route ids and endpoint addresses, never a token. Set
`REPORT_INFERENCE_PROBE=1` to add one unauthenticated endpoint reachability
check.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import urllib.parse

home = pathlib.Path.home()
settings = {}
env_file = home / ".config" / "brama" / "service.env"
if env_file.is_file():
    for line in env_file.read_text(errors="replace").splitlines():
        name, separator, value = line.partition("=")
        if separator and not name.lstrip().startswith("#"):
            settings[name.strip()] = value.strip().strip("'\"")
path = pathlib.Path(
    os.environ.get("BRAMA_INFERENCE_ROUTES_FILE")
    or settings.get("BRAMA_INFERENCE_ROUTES_FILE")
    or (home / ".config" / "brama" / "inference-routes.json")
)

print(f"file: {path}")
if not path.is_file():
    print("state: absent")
    raise SystemExit(0)

try:
    document = json.loads(path.read_text())
except json.JSONDecodeError as error:
    print(f"unreadable: {error}")
    raise SystemExit(1)

if not isinstance(document, dict):
    print(f"unexpected shape: {type(document).__name__}")
    raise SystemExit(1)

deployments = {
    deployment.get("name"): deployment.get("endpoint")
    for deployment in document.get("deployments", [])
    if isinstance(deployment, dict) and isinstance(deployment.get("name"), str)
}
routes = document.get("routes")
fallbacks = document.get("fallbacks", {})
if not isinstance(routes, dict) or not isinstance(fallbacks, dict):
    print("unexpected shape: routes and fallbacks must be objects")
    raise SystemExit(1)

seen: set[str] = set()


def describe(destination: object) -> tuple[str, str]:
    if not isinstance(destination, str) or not destination:
        return (f"invalid={destination!r}", "")
    if "/" in destination:
        return (destination, "")
    endpoint = deployments.get(destination)
    if not isinstance(endpoint, dict):
        return (f"{destination} (unknown deployment)", "")
    host = endpoint.get("host")
    port = endpoint.get("port")
    if not isinstance(host, str) or not isinstance(port, int):
        return (f"{destination} (invalid endpoint)", "")
    return (f"local-openai/{destination}", f"http://{host}:{port}")


def print_destination(label: str, destination: object) -> None:
    resolved, endpoint = describe(destination)
    print(f"    {label}: {destination} -> {resolved}")
    if not endpoint:
        return
    print(f"        endpoint {endpoint}")
    if os.environ.get("REPORT_INFERENCE_PROBE") != "1" or endpoint in seen:
        return
    seen.add(endpoint)
    parsed = urllib.parse.urlsplit(endpoint)
    probe = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, "/", "", ""))
    result = subprocess.run(
        ["/usr/bin/curl", "-s", "-m", "6", "-o", "/dev/null", "-w", "%{http_code}", probe],
        capture_output=True,
        text=True,
    )
    code = result.stdout.strip() or "000"
    print(f"        probe {probe} -> HTTP {code}{'  (nothing listening)' if code == '000' else ''}")


for alias in sorted(routes):
    print(alias)
    print_destination("primary", routes[alias])
    ordered = fallbacks.get(alias, [])
    if not isinstance(ordered, list):
        print(f"    fallbacks: invalid={ordered!r}")
        continue
    for index, destination in enumerate(ordered, start=1):
        print_destination(f"fallback[{index}]", destination)
