#!/usr/bin/env python3
"""Enable Wisent account authentication in the placed Brama service."""

from __future__ import annotations

import os
from pathlib import Path

VALUES = {
    "BRAMA_WISENT_AUTH_URL": "https://alvaewvbyxpgwdpugnxy.supabase.co",
    # Supabase anon keys are public client configuration, not service-role secrets.
    "BRAMA_WISENT_AUTH_ANON_KEY": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImFsdmFld3ZieXhwZ3dkcHVnbnh5Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODEzOTc5NDcsImV4cCI6MjA5Njk3Mzk0N30.xkkJ36ZTwtqyVZLFju0vc9S25grTuKbj9ILKlsXdUPA",
}


def main() -> None:
    path = Path(os.environ.get("BRAMA_SERVICE_ENV_FILE", "~/.config/brama/service.env")).expanduser()
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    retained = [
        line
        for line in lines
        if not any(line.startswith(f"{name}=") for name in VALUES)
    ]
    content = "\n".join([*retained, *(f"{name}={value}" for name, value in VALUES.items())]) + "\n"
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(content, encoding="utf-8")
    temporary.chmod(0o600)
    temporary.replace(path)
    path.chmod(0o600)
    print(f"configured Wisent account authentication in {path}")


if __name__ == "__main__":
    main()
