#!/usr/bin/env python3
"""Report the capability files of the runtime directory in use right now.

The launcher names its runtime directory after the artifact, so a probe with the
old path reports a previous release's state and reads as current. Find the
directory by modification time and print what the gateway is actually holding.

Read-only. Prints names, lengths and digests, never a value.
"""
from __future__ import annotations

import datetime
import hashlib
import json
from pathlib import Path

ROOTS = sorted(
    (path for path in Path("/tmp").glob("brama-skarbiec*") if path.is_dir()),
    key=lambda path: path.stat().st_mtime,
)
if not ROOTS:
    raise SystemExit("no brama runtime directory exists")

for root in ROOTS:
    written = datetime.datetime.fromtimestamp(
        root.stat().st_mtime, datetime.timezone.utc
    ).isoformat()
    print("runtime:", root, "touched:", written)
    for name in ("provider-capabilities.json", "request-sign-capabilities.json"):
        record = root / name
        if not record.exists():
            print("  ", name, "absent")
            continue
        try:
            content = json.loads(record.read_text())
        except ValueError as error:
            print("  ", name, "unreadable:", error)
            continue
        if isinstance(content, dict):
            for key, value in sorted(content.items()):
                text = value if isinstance(value, str) else json.dumps(value)
                digest = hashlib.sha256(text.encode()).hexdigest()[: len("abcdefgh")]
                print(f"   {name}: {key} len={len(text)} digest={digest}")
            if not content:
                print("  ", name, "is empty")
    catalog = root / "subscription-catalog.json"
    if catalog.exists():
        try:
            entries = json.loads(catalog.read_text())
        except ValueError as error:
            print("   subscription-catalog.json unreadable:", error)
        else:
            listed = entries.get("items", entries) if isinstance(entries, dict) else entries
            names = [item.get("id") for item in listed] if isinstance(listed, list) else listed
            print("   subscription-catalog.json:", names)
