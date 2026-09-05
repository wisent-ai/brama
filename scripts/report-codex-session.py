#!/usr/bin/env python3
"""Report whether this host holds a reusable Codex session.

The subscription pool's reauth runner prefers donating an existing
`~/.codex/auth.json` and only drives a login when none can be reused, so the
first question after a provider says "token is expired" is whether a fresher
session is already sitting on the machine that owns the browser.

Read-only, and prints no token: file times, which token fields exist, and the
expiry claim each one carries.
"""

from __future__ import annotations

import base64
import datetime
import json
import os
import pathlib

path = pathlib.Path(os.environ.get("CODEX_AUTH_PATH", pathlib.Path.home() / ".codex/auth.json"))

print(f"file: {path}")
if not path.is_file():
    print("state: absent")
    raise SystemExit(0)

stat = path.stat()
written = datetime.datetime.fromtimestamp(stat.st_mtime, datetime.timezone.utc)
print(f"state: present, {stat.st_size} bytes, written {written.isoformat(timespec='seconds')}")

try:
    document = json.loads(path.read_text())
except json.JSONDecodeError as error:
    print(f"unreadable: {error}")
    raise SystemExit(1)

tokens = document.get("tokens")
if not isinstance(tokens, dict):
    print("carries no tokens object")
    raise SystemExit(0)

print(f"last_refresh: {document.get('last_refresh', '(absent)')}")
now = datetime.datetime.now(datetime.timezone.utc)

for name, value in sorted(tokens.items()):
    if not isinstance(value, str) or not value:
        print(f"{name}: {'empty' if isinstance(value, str) else type(value).__name__}")
        continue
    if value.count(".") != 2:
        print(f"{name}: opaque, length {len(value)}")
        continue
    body = value.split(".")[1]
    try:
        claims = json.loads(base64.urlsafe_b64decode(body + "=" * (-len(body) % 4)))
    except Exception:
        print(f"{name}: undecodable claims, length {len(value)}")
        continue
    expiry = claims.get("exp")
    if not isinstance(expiry, (int, float)):
        print(f"{name}: no exp claim")
        continue
    moment = datetime.datetime.fromtimestamp(expiry, datetime.timezone.utc)
    minutes = int((moment - now).total_seconds() // 60)
    state = "valid" if minutes > 0 else "EXPIRED"
    print(f"{name}: {state}, exp {moment.isoformat(timespec='seconds')} ({minutes:+d} min)")
