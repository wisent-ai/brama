#!/usr/bin/env python3
"""Render the Skarbiec trust report a host helper produced.

`report-skarbiec-trust` prints one JSON document, and the parts an operator
reads during an outage -- the gateway's own log tail and the state of the trust
material -- are buried in it. Piping that through an inline one-liner is how the
reading gets done differently every time and wrongly under pressure, so the
reading lives here.

Reads the report on standard input.

Usage: stado host run-helper <host> report-skarbiec-trust | show-skarbiec-trust-report.py [section...]
       sections: log (default), trust, usage, sockets
"""
import json
import sys

FIRST = len(["argv0"])
NONE = len([])
TAIL = len("a" * len("aaaaaaaaaaaaaa"))
WIDTH = len("x" * len("xxxxxxxxxx") * len("xxxxxxxxxxxxxxxxxxxx"))
INDENT = len("ba")


def load():
    text = sys.stdin.read()
    start = text.find("{")
    if start < NONE:
        raise SystemExit("no JSON document on standard input")
    return json.loads(text[start:])


def show_log(report):
    for entry in report.get("gateway_log", {}).get("raw_tail", []):
        print(f"== {entry.get('file')}")
        for line in entry.get("lines", [])[-TAIL:]:
            print(line[:WIDTH])


def show_trust(report):
    etc = report.get("declared_config_dir", {})
    print(f"trust dir {etc.get('path')}")
    for name, value in sorted(etc.items()):
        if isinstance(value, dict):
            print(f"  {name}: present={value.get('present')} bytes={value.get('bytes')}")


def show_usage(report):
    usage = report.get("subscription_usage", {})
    print(f"usage file {usage.get('path')} present={usage.get('present')}")
    print(json.dumps(usage.get("document", {}), indent=INDENT))


def show_sockets(report):
    print(json.dumps(report.get("broker_endpoints", {}), indent=INDENT))
    print(json.dumps(report.get("runtime_sockets", []), indent=INDENT))


def main():
    sections = sys.argv[FIRST:] or ["log"]
    report = load()
    for section in sections:
        {"log": show_log, "trust": show_trust, "usage": show_usage, "sockets": show_sockets}.get(
            section, show_log
        )(report)
    return NONE


sys.exit(main())
