#!/usr/bin/env python3
"""Ask the running gateway to serve one request per alias and report the code.

`/health` answering says the process started. It says nothing about whether a
provider credential can be redeemed, which is the failure mode this host has
had: healthy and serving nothing. Each alias here is a real completion request
through the loopback listener, so a line with a body means a model answered.

It makes requests and changes nothing.
"""

import json
import os
import pathlib
import subprocess
import urllib.error
import urllib.request

home = pathlib.Path.home()
env_file = home / ".config" / "brama" / "service.env"

settings = {}
for line in env_file.read_text().splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

router = settings.get("ENTITLEMENTS_ROUTER_BIN")
if not router:
    raise SystemExit("service env names no entitlements router")

# The router reads the vault, its trust root and its socket out of the
# environment the launcher builds from this same file. Calling it with only
# the caller's environment is how a probe ends up reporting "vault not
# initialized" about a vault that is perfectly fine.
environment = dict(os.environ)
environment.update(settings)

listed = subprocess.run(
    [router, "get", "echo-model-router"],
    capture_output=True,
    text=True,
    check=False,
    env=environment,
)
try:
    token = json.loads(listed.stdout)["fields"]["token"]
except (ValueError, KeyError):
    raise SystemExit(f"cannot read a bearer from the router: {listed.stderr.strip()}")

# Try loopback first and the address the last boot announced second, taking
# whichever answers. Plain HTTP is refused from anything but loopback, and the
# announcement in the log may name a bind from a previous generation, so
# neither candidate alone is reliable.
ANNOUNCEMENT = "Starting brama server on "
log = pathlib.Path.home() / ".stado" / "logs" / "brama-always-on.err"
announced = ""
if log.is_file():
    for line in log.read_text(errors="replace").splitlines():
        _, marker, remainder = line.partition(ANNOUNCEMENT)
        if marker:
            announced = remainder.strip()
port = settings.get("PORT") or "8080"
candidates = [f"127.0.0.1:{port}"]
if announced:
    candidates.append(announced)

base = ""
for authority in dict.fromkeys(candidates):
    try:
        with urllib.request.urlopen(f"http://{authority}/health") as answer:
            print(f"health {answer.status} at {authority}")
            base = f"http://{authority}"
            break
    except Exception as failure:
        print(f"{authority}: {failure}")
if not base:
    raise SystemExit("no candidate address served /health")

ALIASES = ("-best", "local-openai/chat-primary", "wisent-backend/chat/primary")

for alias in ALIASES:
    payload = json.dumps(
        {"model": alias, "messages": [{"role": "user", "content": "say ok"}]}
    ).encode()
    request = urllib.request.Request(
        f"{base}/v1/chat/completions",
        data=payload,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request) as answer:
            body = json.loads(answer.read())
            choice = next(iter(body.get("choices", [])), {})
            content = choice.get("message", {}).get("content", "").strip()
            served = body.get("model", "")
            print(f"{alias} {answer.status} model={served} said={content!r}")
    except urllib.error.HTTPError as failure:
        detail = failure.read().decode(errors="replace").strip().replace("\n", " ")
        print(f"{alias} {failure.code} {detail}")
    except Exception as failure:
        print(f"{alias} unreachable {failure}")
