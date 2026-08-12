#!/usr/bin/env python3
"""Set the exact Brama control policy for local Cydonia plus Featherless fallback."""

import json
import os
import pathlib
import stat
import tempfile

HOME = pathlib.Path.home()
SERVICE_ENV = HOME / ".config" / "brama" / "service.env"
ALIASES = {
    "-best": "claude-code/claude-opus-4-6",
    "wisent-backend/chat/primary": "featherless/TheDrummer/Cydonia-24B-v4.3",
    "wisent-backend/chat/fallback": "featherless/TheDrummer/Cydonia-24B-v4.3",
    "wisent-backend/evaluation": "openai/default",
    "wisent-backend/embeddings": "openai/embeddings",
    "wisent-backend/moderation": "openai/moderation",
    "weles/agent/primary": "featherless/TheDrummer/Cydonia-24B-v4.3",
}

settings: dict[str, str] = {}
for line in SERVICE_ENV.read_text(encoding="utf-8").splitlines():
    name, separator, value = line.partition("=")
    if separator and not name.lstrip().startswith("#"):
        settings[name.strip()] = value.strip().strip("'\"")

control_path = pathlib.Path(settings.get("BRAMA_CONTROL_CONFIG", ""))
if not control_path.is_file():
    raise SystemExit("BRAMA_CONTROL_CONFIG is not a regular file")

document = json.loads(control_path.read_text(encoding="utf-8"))
brama = document.setdefault("services", {}).setdefault("brama", {})
brama["allowed_models"] = list(ALIASES)
brama["model_aliases"] = ALIASES
brama["required_provider_capabilities"] = ["featherless", "openai"]

fd, temporary = tempfile.mkstemp(prefix=f".{control_path.name}.", dir=control_path.parent)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as target:
        json.dump(document, target, indent=2, sort_keys=True)
        target.write("\n")
        target.flush()
        os.fsync(target.fileno())
    os.chmod(temporary, stat.S_IRUSR | stat.S_IWUSR)
    os.replace(temporary, control_path)
finally:
    if os.path.exists(temporary):
        os.unlink(temporary)

print(f"configured Featherless fallback in {control_path}")
