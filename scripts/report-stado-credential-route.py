#!/usr/bin/env python3
"""Print the nonsecret Stado credential-store route used on this host."""

import json
import os
from pathlib import Path

home = Path(os.environ.get("HOME", "."))
paths = [home / ".config" / "stado" / "config.json", home / ".stado" / "config.json"]
for path in paths:
    if not path.is_file():
        continue
    document = json.loads(path.read_text(encoding="utf-8"))
    print("config:", path)
    print("credentials:", json.dumps(document.get("credentials"), sort_keys=True))
    print("secrets.skarbiec:", json.dumps((document.get("secrets") or {}).get("skarbiec"), sort_keys=True))
print("environment:")
for name in ("STADO_CREDENTIALS_STORE", "WC_SKARBIEC_URL", "WC_SKARBIEC_CONSUMER", "WC_SKARBIEC_TOKEN_FILE"):
    value = os.environ.get(name, "")
    print(f"  {name}={value}")
