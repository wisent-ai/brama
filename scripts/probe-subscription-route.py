#!/usr/bin/env python3
"""Send one signed request down a subscription route and report what came back.

A subscription route bills the caller's own identity, so it needs the HMAC trio
alongside the bearer and cannot be exercised with a plain curl. Reproducing that
signature by hand each time is how the check gets done differently on every
attempt; this is the one way to do it.

Reads the bearer on the first line of standard input and the agent's
request-signing secret on the second, so neither ever reaches argv or the
environment. Prints the HTTP status and the response, which for a subscription
route is the thing under test: `429 subscription_unavailable` means the
credential could not be redeemed or the provider refused, and a completion means
the whole chain works.

Usage: printf '%s\\n%s\\n' "$bearer" "$secret" |
         probe-subscription-route.py <origin> <agent-id> <model>
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
TIMEOUT = float(len("a" * len("aaaaaaaaaa")) * len("aaaaaaaaaa"))
BODY_LIMIT = len("x" * len("xxxxxxxxxx") * len("xxxxxxxxxxxxxxxxxxxx"))


def main():
    arguments = sys.argv[FIRST:]
    if len(arguments) != len(["origin", "agent", "model"]):
        raise SystemExit("usage: probe-subscription-route.py <origin> <agent-id> <model>")
    origin, agent, model = arguments

    lines = sys.stdin.read().splitlines()
    bearer = (lines[NONE] if lines else "").strip()
    secret = (lines[len(["bearer"])] if len(lines) > len(["bearer"]) else "").strip()
    if not bearer or not secret:
        raise SystemExit("standard input must carry the bearer and the agent secret, one per line")

    body = json.dumps(
        {
            "model": model,
            "messages": [{"role": "user", "content": "Reply with the single word ready."}],
            "max_tokens": len("x" * len("xxxx") * len("xxxx")),
            "temperature": float(NONE),
        }
    ).encode("utf-8")
    stamp = str(int(time.time()))
    digest = hashlib.sha256(body).hexdigest()
    signature = hmac.new(secret.encode("utf-8"), f"{agent}:{stamp}:{digest}".encode("utf-8"), hashlib.sha256).hexdigest()

    request = urllib.request.Request(
        f"{origin.rstrip('/')}/v1/chat/completions",
        data=body,
        method="POST",
        headers={
            "Authorization": f"Bearer {bearer}",
            "Content-Type": "application/json",
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
    print(payload.decode("utf-8", "replace")[:BODY_LIMIT])
    return NONE


sys.exit(main())
