"""Print the field names of the vault items a Brama host may route.

Companion to report-service-readiness.sh. A capability route names one item
and one field, and the field is the part that cannot be guessed from the
resource id: `provider:openai` says nothing about whether the key sits in
`api_key`, `token` or `value`. Choosing wrong hands a purpose a credential the
operator never meant it to spend, so the names are read here and the choice is
made with them in view.

Names only. No value from the vault is read or printed.
"""

from __future__ import annotations

import json
import subprocess
import sys


def entries(document: object) -> list[dict]:
    if isinstance(document, list):
        return [item for item in document if isinstance(item, dict)]
    if isinstance(document, dict):
        for key in ("items", "subscriptions", "entries"):
            value = document.get(key)
            if isinstance(value, list):
                return [item for item in value if isinstance(item, dict)]
    return []


def field_names(entry: dict) -> list[str]:
    names: set[str] = set()
    candidates = [entry.get("fields")]
    versions = entry.get("versions")
    if isinstance(versions, list):
        for version in versions:
            if isinstance(version, dict):
                candidates.append(version.get("fields"))
                candidates.append(version.get("data"))
                candidates.append(version.get("secrets"))
    elif isinstance(versions, dict):
        for version in versions.values():
            if isinstance(version, dict):
                candidates.append(version.get("fields"))
                candidates.append(version.get("data"))
                candidates.append(version.get("secrets"))
    for candidate in candidates:
        if isinstance(candidate, dict):
            names.update(candidate)
        elif isinstance(candidate, list):
            names.update(str(name) for name in candidate)
    return sorted(names)


def version_shape(entry: dict) -> list[str]:
    versions = entry.get("versions")
    if isinstance(versions, list):
        for version in versions:
            if isinstance(version, dict):
                return sorted(version)
    if isinstance(versions, dict):
        for version in versions.values():
            if isinstance(version, dict):
                return sorted(version)
    return []


def inventory() -> object:
    # `--vault <stado> <vault>` asks Stado for the vault's nonsecret metadata
    # instead of reading a file the router happened to leave behind. Running it
    # here keeps the failure text: a shell cannot merge the error stream
    # without naming a file descriptor, and the reason it refused is the only
    # useful part when it refuses.
    if "--vault" in sys.argv:
        vault = sys.argv.pop()
        binary = sys.argv.pop()
        result = subprocess.run(
            [binary, "credentials", "inspect-vault", vault, "--json"],
            capture_output=True,
            text=True,
        )
        if result.returncode:
            detail = (result.stderr or result.stdout).strip()
            print("inspect-vault failed:", detail)
            return None
        return json.loads(result.stdout)
    path = sys.argv.pop()
    with open(path, encoding="utf-8") as source:
        return json.load(source)


def main() -> None:
    document = inventory()
    if document is None:
        return

    listed = entries(document)
    if not listed:
        print("no entries")
        return

    print("entry keys:", ",".join(sorted(next(iter(listed)))))
    for entry in listed:
        identifier = entry.get("id")
        if not isinstance(identifier, str):
            continue
        if not identifier.startswith("provider:") and not identifier.startswith("agent:"):
            continue
        names = field_names(entry)
        print(identifier, "fields:", ",".join(names) if names else "unlisted")
        print("   version keys:", ",".join(version_shape(entry)) or "none")


if __name__ == "__main__":
    raise SystemExit(main())
