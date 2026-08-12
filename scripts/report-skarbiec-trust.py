#!/usr/bin/env python3
"""Report the Skarbiec trust material an installed Brama actually holds.

Capability issuance and redemption fail closed, and both failures read the same
way from a caller: `credential unavailable`. The difference is on disk — the
policy, the workload registry, the proof key, and the WORM receipt that
`provision-skarbiec-trust` writes once per installation. This prints which of
them exist, their sizes and modes, and the runtime directory the launcher is
using, so the missing piece is named instead of guessed.

Read-only. Prints no secret material: names, sizes, modes and mtimes only.
"""
import json
import os
import stat
from datetime import datetime, timezone
from pathlib import Path

HOME = Path.home()
SERVICE = HOME / ".stado" / "services" / "brama" / "current" / "darwin-arm"
TRUST_NAMES = [
    "policy.json",
    "policy.sig",
    "registry.json",
    "registry.sig",
    "trust.json",
    "brama-proof.key",
    "worm-receipt",
    "subscriptions.json",
    "recipient-public-keys.asc",
]


def describe(path: Path) -> dict:
    try:
        info = path.stat()
    except OSError as error:
        return {"present": False, "detail": str(error)}
    return {
        "present": True,
        "bytes": info.st_size,
        "mode": stat.filemode(info.st_mode),
        "modified": datetime.fromtimestamp(info.st_mtime, timezone.utc).isoformat(),
    }


def main() -> None:
    report = {
        "service_root": str(SERVICE),
        "service_root_present": SERVICE.is_dir(),
        "etc": {},
        "config_dir": {},
        "env_file": {},
    }

    etc = SERVICE / "etc" / "brama-skarbiec"
    report["etc"]["path"] = str(etc)
    report["etc"]["entries"] = sorted(p.name for p in etc.iterdir()) if etc.is_dir() else []
    for name in TRUST_NAMES:
        report["etc"][name] = describe(etc / name)

    config_dir = HOME / ".config" / "brama"
    report["config_dir"]["path"] = str(config_dir)
    report["config_dir"]["entries"] = (
        sorted(p.name for p in config_dir.iterdir()) if config_dir.is_dir() else []
    )
    report["config_dir"]["gnupg_present"] = (config_dir / "gnupg").is_dir()
    report["config_dir"]["proof_key"] = describe(config_dir / "brama-proof.key")

    env_file = config_dir / "service.env"
    report["env_file"]["path"] = str(env_file)
    if env_file.is_file():
        names = []
        for line in env_file.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            names.append(line.split("=", 1)[0])
        report["env_file"]["present"] = True
        report["env_file"]["variables"] = sorted(names)
    else:
        report["env_file"]["present"] = False

    runtimes = sorted(
        (p for p in Path("/tmp").glob("brama-skarbiec-*") if p.is_dir()),
        key=lambda p: p.stat().st_mtime,
    )
    report["runtime_directories"] = [
        {
            "path": str(p),
            "touched": datetime.fromtimestamp(p.stat().st_mtime, timezone.utc).isoformat(),
            "entries": sorted(entry.name for entry in p.iterdir()),
        }
        for p in runtimes[-3:]
    ]
    settings = {}
    if env_file.is_file():
        for line in env_file.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            name, _, value = line.partition("=")
            settings[name.strip()] = value.strip().strip('"')

    declared = settings.get("BRAMA_SKARBIEC_CONFIG_DIR", "")
    declared_dir = Path(os.path.expandvars(declared)).expanduser() if declared else None
    report["declared_config_dir"] = {"path": str(declared_dir) if declared_dir else None}
    if declared_dir and declared_dir.is_dir():
        report["declared_config_dir"]["entries"] = sorted(p.name for p in declared_dir.iterdir())
        for name in TRUST_NAMES:
            report["declared_config_dir"][name] = describe(declared_dir / name)
    else:
        report["declared_config_dir"]["entries"] = []

    for key in ("SKARBIEC_VAULT_FILE", "SKARBIEC_CAPABILITY_ROUTES_FILE", "BRAMA_GNUPG_HOME"):
        value = settings.get(key, "")
        target = Path(os.path.expandvars(value)).expanduser() if value else None
        report[key] = {"path": str(target) if target else None, **(describe(target) if target else {})}

    vault_path = report.get("SKARBIEC_VAULT_FILE", {}).get("path")
    vault_report = {"path": vault_path, "items": []}
    if vault_path and Path(vault_path).is_file():
        try:
            document = json.loads(Path(vault_path).read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            vault_report["detail"] = str(error)
        else:
            raw = document.get("items", [])
            # The vault keys items by an opaque uuid and carries the caller-facing
            # name inside the record, so both shapes are read: a list from older
            # documents, a map from current ones.
            entries = list(raw.values()) if isinstance(raw, dict) else raw
            for entry in entries if isinstance(entries, list) else []:
                if not isinstance(entry, dict):
                    continue
                identifier = str(entry.get("id") or entry.get("name") or "")
                if not identifier.startswith("provider:"):
                    continue
                # Ids, recipients and field names only. Values stay encrypted and
                # are never read here: the question is which items exist and who
                # can open them, not what they hold.
                vault_report["items"].append(
                    {
                        "id": identifier,
                        "deleted": bool(entry.get("deleted", False)),
                        "kind": entry.get("kind"),
                        "recipients": entry.get("recipients", []),
                        "fields": sorted((entry.get("fields") or {}).keys())
                        if isinstance(entry.get("fields"), dict)
                        else [],
                        "updated_at": entry.get("updated_at"),
                    }
                )
            vault_report["item_count"] = len(entries) if isinstance(entries, list) else 0
            vault_report["sample_keys"] = sorted(
                {key for entry in (entries or [])[:5] if isinstance(entry, dict) for key in entry}
            )
    report["vault_provider_items"] = vault_report

    usage_file = settings.get("BRAMA_SUBSCRIPTION_USAGE_FILE") or str(
        config_dir / "subscription-usage.json"
    )
    usage_path = Path(os.path.expandvars(usage_file)).expanduser()
    report["subscription_usage"] = {"path": str(usage_path), **describe(usage_path)}
    if usage_path.is_file():
        try:
            report["subscription_usage"]["document"] = json.loads(
                usage_path.read_text(encoding="utf-8")
            )
        except ValueError as error:
            report["subscription_usage"]["detail"] = str(error)

    # The gateway's own tracing output, filtered. A capability refusal names the
    # resource and the authority's reason, and reading it is the difference
    # between repairing a grant and guessing at one.
    log_root = HOME / ".stado" / "logs"
    interesting = (
        "capability",
        "credential_",
        "subscription_",
        "redeem",
        "codex",
    )
    lines = []
    for log in sorted(log_root.glob("brama*.err"), key=lambda p: p.stat().st_mtime)[-2:]:
        text = log.read_text(encoding="utf-8", errors="replace").splitlines()
        lines.extend(line for line in text if any(token in line for token in interesting))
    report["gateway_log"] = {"root": str(log_root), "matching_tail": lines[-400:]}
    newest = sorted(log_root.glob("brama*"), key=lambda p: p.stat().st_mtime)[-1:]
    report["gateway_log"]["raw_tail"] = [
        {
            "file": str(log),
            "lines": log.read_text(encoding="utf-8", errors="replace").splitlines()[-25:],
        }
        for log in newest
    ]

    # Issuing and redeeming must reach the same broker. Each release start makes
    # a fresh runtime directory with its own socket, so a stale path in the
    # service env is the exact shape of "no such capability": the router issues
    # against one instance and the client redeems against another.
    sockets = {
        "SKARBIEC_CAP_SOCKET": settings.get("SKARBIEC_CAP_SOCKET", ""),
        "SKARBIEC_SOCKET": settings.get("SKARBIEC_SOCKET", ""),
        "WC_SKARBIEC_URL": settings.get("WC_SKARBIEC_URL", ""),
    }
    report["broker_endpoints"] = {
        name: {
            "value": value,
            **(describe(Path(os.path.expandvars(value)).expanduser()) if value.startswith("/") else {}),
        }
        for name, value in sockets.items()
    }
    report["runtime_sockets"] = [
        {"path": str(p / "socket"), **describe(p / "socket")}
        for p in sorted(Path("/tmp").glob("brama-skarbiec-*"), key=lambda p: p.stat().st_mtime)[-3:]
    ]


    launcher = HOME / ".stado" / "bin" / "skarbiec-keychain-launcher"
    report["keychain_launcher"] = describe(launcher)
    report["skarbiec_socket"] = describe(Path(os.environ.get("SKARBIEC_SOCKET", "/tmp/skarbiec.sock")))

    print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
