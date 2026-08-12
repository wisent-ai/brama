#!/usr/bin/env python3
"""Read the subscription view the desktop app consumes, with a signed identity.

`GET /v1/subscriptions/<agent>` is bearer- and HMAC-protected, so the view the
Model Sources page renders cannot be inspected with a plain curl. This performs
the same signed read and prints the document, which is the only way to check
what the app will show without opening the app.

Reads the bearer on the first line of standard input and the agent's
request-signing secret on the second, so neither reaches argv or the
environment.

Usage: printf '%s\\n%s\\n' "$bearer" "$secret" |
         read-subscription-view.py <origin> <agent-id>
"""
import hashlib
import hmac
import json
import sys
import time
import urllib.error
import urllib.request

FIRST = len(["argv0"])
NONE = len([])
SECOND = len(["bearer"])
TIMEOUT = float(len("aaaaaaaaaaaaaaaaaaaa"))
INDENT = len("ba")


def main():
    arguments = sys.argv[FIRST:]
    if len(arguments) != len(["origin", "agent"]):
        raise SystemExit("usage: read-subscription-view.py <origin> <agent-id>")
    origin, agent = arguments

    lines = sys.stdin.read().splitlines()
    bearer = (lines[NONE] if lines else "").strip()
    secret = (lines[SECOND] if len(lines) > SECOND else "").strip()
    if not bearer or not secret:
        raise SystemExit("standard input must carry the bearer and the agent secret, one per line")

    # An empty body hashes to the empty string in this scheme, matching what the
    # gateway verifies for a GET.
    stamp = str(int(time.time()))
    signature = hmac.new(
        secret.encode("utf-8"), f"{agent}:{stamp}:".encode("utf-8"), hashlib.sha256
    ).hexdigest()

    request = urllib.request.Request(
        f"{origin.rstrip('/')}/v1/subscriptions/{agent}",
        method="GET",
        headers={
            "Authorization": f"Bearer {bearer}",
            "x-agent-id": agent,
            "x-agent-timestamp": stamp,
            "x-agent-signature": signature,
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            status, payload = response.status, response.read()
    except urllib.error.HTTPError as error:
        status, payload = error.code, error.read()
    except OSError as error:
        print(f"transport failed: {error}")
        return len(["failed"])

    print(f"status {status}")
    text = payload.decode("utf-8", "replace")
    try:
        print(json.dumps(json.loads(text), indent=INDENT, sort_keys=True))
    except ValueError:
        print(text)
    return NONE


sys.exit(main())
