#!/usr/bin/env python3
"""Print bounded recent Weles routing and provider credential events."""
import json
import re
from pathlib import Path

path = Path.home() / ".stado" / "logs" / "brama-always-on.err"
events = []
ansi = re.compile(r"\x1b\[[0-9;]*m")
for line in path.read_text(errors="replace").splitlines():
    clean = ansi.sub("", line)
    if (
        "weles/agent/primary" not in clean
        and '"weles"' not in clean
        and "local-openai" not in clean
        and 'event="provider_credential_' not in clean
        and 'event="credential_read_' not in clean
    ):
        continue
    event = {"line": clean[-800:]}
    for key in ("event", "provider", "requested_model", "selected_model", "resource", "detail", "error", "error_code", "success", "client_id", "agent_id"):
        match = re.search(rf"{key}=(?:\"([^\"]*)\"|(\S+))", clean)
        if match:
            event[key] = (match.group(1) or match.group(2))[:160]
    events.append(event)
print(json.dumps(events[-30:], indent=2))
