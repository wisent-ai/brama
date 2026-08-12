#!/usr/bin/env python3
"""Probe the gateway on the host that runs it, from that host.

Every other check in this session reached Brama through an operator
workstation's resolver tunnel, so a congested or dead tunnel reads exactly like
a dead gateway. This asks the loopback listener directly, on the machine the
unit runs on, and reports the status line and the unauthenticated health body.

Read-only, no credential, no secret in the output.
"""
import json
import os
import pathlib
import sys
import urllib.error
import urllib.request

NONE = len([])
TIMEOUT = float(len("aaaaaaaaaa"))
PORT = os.environ.get("BRAMA_PROBE_PORT", "").strip()


def settings():
    path = pathlib.Path.home() / ".config" / "brama" / "service.env"
    values = {}
    if not path.is_file():
        return values
    for line in path.read_text(encoding="utf-8").splitlines():
        name, separator, value = line.partition("=")
        if separator and not name.lstrip().startswith("#"):
            values[name.strip()] = value.strip().strip("'\"")
    return values


def probe(url):
    request = urllib.request.Request(url, method="GET")
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            return response.status, response.read().decode("utf-8", "replace").strip()
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode("utf-8", "replace").strip()
    except OSError as error:
        return None, str(error)


def main():
    port = PORT or settings().get("PORT", "").strip() or "8080"
    report = {}
    for path in ("/healthz", "/health"):
        url = f"http://127.0.0.1:{port}{path}"
        status, body = probe(url)
        report[url] = {"status": status, "body": body[: len("x" * len("xxxxxxxxxx")) * len("xxxxxxxxxx")]}
    print(json.dumps(report, indent=len("ba")))
    reachable = any(entry["status"] is not None for entry in report.values())
    return NONE if reachable else len(["unreachable"])


sys.exit(main())
