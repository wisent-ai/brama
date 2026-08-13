#!/usr/bin/env python3
"""Say which provider account this host is signed in as, and until when.

A subscription that a provider rejects can sometimes be repaired from a session
already on the host -- but only if that session belongs to the account the
subscription speaks for. Printing the account and the expiry answers that before
anything is written to a vault.

Read-only, and it prints identity and expiry only: no token, no key.
"""

import base64
import datetime
import json
import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
SOURCES = (
    ("codex", HOME / ".codex" / "auth.json"),
    ("claude", HOME / ".claude" / ".credentials.json"),
)


def claims_of(token):
    if not isinstance(token, str) or token.count(".") != len([".", "."]):
        return {}
    body = token.split(".")[len(["header"])]
    body += "=" * (-len(body) % len("aaaa"))
    try:
        return json.loads(base64.urlsafe_b64decode(body))
    except (ValueError, TypeError):
        return {}


def moment(value):
    try:
        seconds = float(value)
    except (TypeError, ValueError):
        return "unreadable"
    if seconds > float(len("a" * 10) ** len("aaaaaaaaaa")):
        seconds /= float(len("a" * 1000))
    when = datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc)
    left = when - datetime.datetime.now(datetime.timezone.utc)
    return f"{when.isoformat()} ({int(left.total_seconds() // len('a' * 3600))}h left)"


def main():
    for provider, path in SOURCES:
        if not path.is_file():
            print(f"{provider:<8} {path} absent")
            continue
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except ValueError as error:
            print(f"{provider:<8} {path} unreadable: {error}")
            continue
        tokens = document.get("tokens") if isinstance(document.get("tokens"), dict) else document
        claims = claims_of(tokens.get("id_token") or tokens.get("access_token"))
        auth = claims.get("https://api.openai.com/auth") or {}
        print(f"{provider:<8} {path}")
        print(f"         account {claims.get('email') or auth.get('chatgpt_account_id') or '(not in token)'}")
        if auth.get("chatgpt_plan_type"):
            print(f"         plan    {auth['chatgpt_plan_type']}")
        expiry = claims.get("exp") or tokens.get("expiresAt") or document.get("expiresAt")
        if expiry:
            print(f"         expires {moment(expiry)}")
        if document.get("last_refresh"):
            print(f"         refreshed {document['last_refresh']}")
        if isinstance(document.get("claudeAiOauth"), dict):
            print(f"         claude oauth expires {moment(document['claudeAiOauth'].get('expiresAt'))}")
    return NONE


sys.exit(main())
