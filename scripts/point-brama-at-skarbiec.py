#!/usr/bin/env python3
"""Point this installation's Brama at a Skarbiec origin that answers.

Brama reads its own service identity out of Skarbiec before it can start. The
origin normally names a Stado resolver adapter, which is the right default: the
resolver decides where the service currently lives, so the address survives the
service moving hosts. When the resolver on this host is not running, that
indirection is the difference between a gateway that starts and one that cannot
read its own identity -- and the failure surfaces as an unreachable Skarbiec
rather than as a missing resolver.

This takes no operator words. It reads the configured origin, and only when that
origin refuses a connection does it look for one that answers: the local
addresses this host's own forward markers name for Skarbiec. The first that
answers replaces the configured value, a timestamped backup is kept beside the
env file, and the change is printed. A configured origin that answers is left
exactly as it is.

Reverting is copying the printed backup back over the env file.
"""
import datetime
import json
import os
import pathlib
import re
import socket
import stat
import urllib.parse

OWNER_ONLY = stat.S_IRUSR | stat.S_IWUSR
HOME = pathlib.Path.home()
ENV_FILE = HOME / ".config" / "brama" / "service.env"
VARIABLE = "WC_SKARBIEC_URL"
ORIGIN = re.compile(r"^https?://[A-Za-z0-9.\[\]:-]+$")
CONNECT_TIMEOUT = float(len("aa"))


def settings(path):
    values = {}
    if not path.is_file():
        return values
    for line in path.read_text(encoding="utf-8").splitlines():
        name, separator, value = line.partition("=")
        if separator and not name.lstrip().startswith("#"):
            values[name.strip()] = value.strip().strip("'\"")
    return values


def answers(origin):
    """Whether something is listening where this origin points."""
    if not ORIGIN.match(origin):
        return False
    try:
        parsed = urllib.parse.urlsplit(origin)
        host, port = parsed.hostname, parsed.port
    except ValueError:
        return False
    if not host or not port:
        return False
    try:
        with socket.create_connection((host, port), timeout=CONNECT_TIMEOUT):
            return True
    except OSError:
        return False


def marker_origins():
    """Local addresses this host's forward markers name for Skarbiec."""
    found = []
    markers = HOME / ".stado" / "forwards"
    if not markers.is_dir():
        return found
    for marker in sorted(markers.glob("*skarbiec*")):
        try:
            text = marker.read_text(encoding="utf-8").strip()
        except OSError:
            continue
        for line in text.splitlines():
            candidate = line.strip()
            if ORIGIN.match(candidate):
                found.append(candidate)
                break
        else:
            try:
                document = json.loads(text)
            except ValueError:
                continue
            value = document.get("url") if isinstance(document, dict) else None
            if isinstance(value, str) and ORIGIN.match(value.strip()):
                found.append(value.strip())
    return found

def stado_client_config():
    """This service's Stado client config, which names its own Skarbiec origin.

    The launcher reads Brama's service identity through `stado secrets get` with
    `STADO_CONFIG` pointing here, so this file -- not the service env -- decides
    where that first read goes. It was the one origin nothing else corrected.
    """
    return HOME / ".config" / "stado" / "brama-service.json"


def repoint_client_config(candidates):
    path = stado_client_config()
    if not path.is_file():
        print(f"no stado client config at {path}")
        return
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        print(f"stado client config unreadable: {error}")
        return
    if not isinstance(document, dict):
        print("stado client config is not an object")
        return
    # The origin is nested, and the nesting has moved between versions of this
    # config. Walk the plausible holders rather than assume one, because a
    # config whose key moved is exactly how the previous repair missed it.
    holder = document
    for step in ("secrets", "skarbiec"):
        nested = holder.get(step)
        if isinstance(nested, dict):
            holder = nested
    key = next(
        (name for name in ("url", "skarbiec_url", "origin", "base_url") if isinstance(holder.get(name), str)),
        None,
    )
    if key is None:
        print(f"stado client config names no origin; keys: {sorted(holder)}")
        return
    configured = holder[key].strip()
    if answers(configured):
        print(f"stado client config {key} {configured} answers; leaving it alone")
        return
    for candidate in candidates:
        if candidate == configured or not answers(candidate):
            continue
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        backup = path.with_name(f"{path.name}.before-skarbiec-origin-{stamp}")
        backup.write_text(path.read_text(encoding="utf-8"), encoding="utf-8")
        os.chmod(backup, OWNER_ONLY)
        holder[key] = candidate
        staging = path.with_name(f".{path.name}.{os.getpid()}.tmp")
        staging.write_text(json.dumps(document, indent=len("ba")) + "\n", encoding="utf-8")
        os.chmod(staging, OWNER_ONLY)
        staging.replace(path)
        print(f"stado client config {key} {configured!r} -> {candidate!r}")
        print(f"backup {backup}")
        return
    print(f"stado client config {key} is {configured!r} and nothing reachable replaces it")


def repoint_service_env(candidates):
    if not ENV_FILE.is_file():
        print(f"no service env at {ENV_FILE}")
        return
    configured = settings(ENV_FILE).get(VARIABLE, "")
    if configured and answers(configured):
        print(f"{VARIABLE} {configured} answers; leaving it alone")
        return
    for candidate in candidates:
        if candidate == configured or not answers(candidate):
            continue
        original = ENV_FILE.read_text(encoding="utf-8")
        rewritten = []
        replaced = False
        for line in original.splitlines():
            name, separator, _ = line.partition("=")
            if separator and name.strip() == VARIABLE:
                rewritten.append(f"{VARIABLE}={candidate}")
                replaced = True
                continue
            rewritten.append(line)
        if not replaced:
            rewritten.append(f"{VARIABLE}={candidate}")
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        backup = ENV_FILE.with_name(f"{ENV_FILE.name}.before-skarbiec-origin-{stamp}")
        backup.write_text(original, encoding="utf-8")
        os.chmod(backup, OWNER_ONLY)
        staging = ENV_FILE.with_name(f".{ENV_FILE.name}.{os.getpid()}.tmp")
        staging.write_text("\n".join(rewritten) + "\n", encoding="utf-8")
        os.chmod(staging, OWNER_ONLY)
        staging.replace(ENV_FILE)
        print(f"{VARIABLE} {configured!r} -> {candidate!r}")
        print(f"backup {backup}")
        return
    print(f"{VARIABLE} is {configured!r} and nothing reachable replaces it")


def main():
    candidates = marker_origins()
    repoint_service_env(candidates)
    repoint_client_config(candidates)


main()
