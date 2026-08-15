#!/usr/bin/env python3
"""Configure Wisent account authentication and durable Brama trust state."""

from __future__ import annotations

import os
import shutil
from pathlib import Path

AUTH_VALUES = {
    "BRAMA_WISENT_AUTH_URL": "https://alvaewvbyxpgwdpugnxy.supabase.co",
    # Supabase anon keys are public client configuration, not service-role secrets.
    "BRAMA_WISENT_AUTH_ANON_KEY": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImFsdmFld3ZieXhwZ3dkcHVnbnh5Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODEzOTc5NDcsImV4cCI6MjA5Njk3Mzk0N30.xkkJ36ZTwtqyVZLFju0vc9S25grTuKbj9ILKlsXdUPA",
}


def main() -> None:
    path = Path(os.environ.get("BRAMA_SERVICE_ENV_FILE", "~/.config/brama/service.env")).expanduser()
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    configured = {
        name: value.strip().strip("\"'")
        for line in lines
        if "=" in line
        for name, value in [line.split("=", 1)]
    }

    trust_dir = Path.home() / ".config" / "brama" / "trust"
    configured_source = configured.get("BRAMA_SKARBIEC_CONFIG_DIR")
    candidate_dirs = []
    if configured_source:
        candidate_dirs.append(Path(configured_source).expanduser())
    candidate_dirs.extend(
        sorted(
            (
                manifest.parent
                for manifest in (Path.home() / ".stado" / "services" / "brama").glob(
                    "*/darwin-arm/etc/brama-skarbiec/recipient-public-keys.asc"
                )
            ),
            key=lambda candidate: (candidate / "recipient-public-keys.asc").stat().st_mtime,
            reverse=True,
        )
    )
    # Only the recipient keys are release content now; the subscriptions manifest
    # is gone, because what exists is read off the vault items themselves.
    for manifest_name in ("recipient-public-keys.asc",):
        target = trust_dir / manifest_name
        if target.exists():
            continue
        source = next(
            (
                candidate / manifest_name
                for candidate in candidate_dirs
                if (candidate / manifest_name).is_file()
            ),
            None,
        )
        if source is None:
            raise SystemExit(f"cannot locate the existing Brama {manifest_name}")
        trust_dir.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        target.chmod(0o600)

    values = {
        **AUTH_VALUES,
        "BRAMA_SKARBIEC_CONFIG_DIR": str(trust_dir),
    }
    retained = [
        line
        for line in lines
        if not any(line.startswith(f"{name}=") for name in values)
    ]
    content = "\n".join([*retained, *(f"{name}={value}" for name, value in values.items())]) + "\n"
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(content, encoding="utf-8")
    temporary.chmod(0o600)
    temporary.replace(path)
    path.chmod(0o600)
    print(f"configured Wisent account authentication and durable trust state in {path}")


if __name__ == "__main__":
    main()
