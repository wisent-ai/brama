#!/usr/bin/env python3
"""Ask the broker for a request-sign capability the way the gateway does.

The gateway reports `no auth secret for agent` and nothing else: the failure is
inside issuing or redeeming a capability whose purpose is request-signing, and
neither step logs when it refuses. Issuing one here, with the same purpose,
resource and target the gateway uses, turns that silence into the authority's
own sentence.

A capability is short-lived and single-use by contract, so asking for one
without redeeming it changes nothing a later request depends on.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
ROUTER = HOME / ".stado" / "bin" / "skarbiec"
PURPOSE = os.environ.get("REQUEST_SIGN_PURPOSE", "brama.request.sign")
RESOURCE = os.environ.get("REQUEST_SIGN_RESOURCE", "agent:wisent-app")
AGENT = os.environ.get("REQUEST_SIGN_AGENT", "brama-runtime")
TARGET = os.environ.get("REQUEST_SIGN_TARGET", "brama")

runtimes = sorted(
    (path for path in Path("/tmp").glob("brama-skarbiec*") if path.is_dir()),
    key=lambda path: path.stat().st_mtime,
)
if not runtimes:
    raise SystemExit("no brama runtime directory exists")
runtime = runtimes[len(runtimes) - len("x")]
config_dirs = sorted(
    (HOME / ".stado" / "services" / "brama").glob("*/darwin-arm/etc/brama-skarbiec")
)
policy = next((path / "policy.json" for path in config_dirs if (path / "policy.json").exists()), None)

print("runtime:", runtime)
print("policy:", policy)
if policy is not None:
    roles = json.loads(policy.read_text()).get("roles", {})
    rules = roles.get(AGENT, [])
    matching = [rule for rule in rules if rule.get("resource") == RESOURCE]
    print("policy rules for", RESOURCE, ":", json.dumps(matching, sort_keys=True))

# The broker looks for the routes table beside the capability state unless the
# service environment names it, so a probe that omits that variable reproduces a
# default the gateway never uses and reports a missing route that exists.
service_env = Path(
    os.environ.get(
        "BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env")
    )
)
declared = {}
if service_env.exists():
    for line in service_env.read_text().splitlines():
        name, separator, value = line.partition("=")
        if separator and not line.lstrip().startswith("#"):
            declared[name.strip()] = value.strip()
routes_file = declared.get("SKARBIEC_CAPABILITY_ROUTES_FILE", "")
print("routes file from service env:", routes_file or "(absent)")

issued = subprocess.run(
    [
        str(ROUTER),
        "capability-issue",
        "--agent", AGENT,
        "--purpose", PURPOSE,
        "--resource", RESOURCE,
        "--target", TARGET,
    ],
    capture_output=True,
    text=True,
    env={
        **os.environ,
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "SKARBIEC_CAP_STATE": str(runtime / "capability.sqlite"),
        "SKARBIEC_CAP_SOCKET": str(runtime / "socket" / "broker.sock"),
        **({"SKARBIEC_CAPABILITY_ROUTES_FILE": routes_file} if routes_file else {}),
    },
)
print("issue exit:", issued.returncode)
print("  out:", " ".join((issued.stdout or "").split()))
print("  err:", " ".join((issued.stderr or "").split()))
