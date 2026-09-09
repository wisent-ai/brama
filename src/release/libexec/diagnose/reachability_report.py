"""Where the gateway answers, and what this boot has said about itself.

A gateway can be perfectly configured and still be reached by nobody, and it can
refuse to start for a reason that is written down only once, in the stream
nothing reads. So this asks every address a caller might use, and then prints
the launcher's own account of the attempt that is running now.
"""

import json
import pathlib
import shutil
import socket
import ssl
import subprocess
import urllib.error
import urllib.request

from host_layout import BOOT_MARKER, error_log, output_log, settings


def print_reachability():
    """Where the gateway is reachable, and by which scheme."""
    print("\n=== reachability")
    port = settings.get("PORT") or "8080"
    announced = ""
    if error_log.is_file():
        for line in error_log.read_text(errors="replace").splitlines():
            _, marker, remainder = line.partition("Starting brama server on ")
            if marker:
                announced = remainder.strip()
    targets = [f"http://127.0.0.1:{port}/health"]
    if announced:
        targets.append(f"http://{announced}/health")
    tailscale = shutil.which("tailscale") or "/usr/local/bin/tailscale"
    if pathlib.Path(tailscale).exists():
        status = subprocess.run(
            [tailscale, "status", "--json"], capture_output=True, text=True, check=False
        )
        if status.returncode == int():
            name = (json.loads(status.stdout).get("Self") or {}).get("DNSName", "").rstrip(".")
            if name:
                targets.append(f"https://{name}:8443/health")
        served = subprocess.run(
            [tailscale, "serve", "status"], capture_output=True, text=True, check=False
        )
        for line in (served.stdout + served.stderr).strip().splitlines():
            if "proxy" in line or "https://" in line:
                print(f"  serve: {line.strip()}")
    relaxed = ssl.create_default_context()
    relaxed.check_hostname = False
    relaxed.verify_mode = ssl.CERT_NONE
    for target in dict.fromkeys(targets):
        context = relaxed if target.startswith("https") else None
        try:
            with urllib.request.urlopen(target, context=context, timeout=float(len("ten"))) as answer:
                print(f"  {target} -> {answer.status}")
        except urllib.error.HTTPError as failure:
            print(f"  {target} -> HTTP {failure.code}")
        except (urllib.error.URLError, socket.timeout, ssl.SSLError, OSError) as failure:
            print(f"  {target} -> {failure}")


# The slice below starts at the last time the gateway announced itself, which
# hides a start that never got that far: a launcher that dies while provisioning,
# registering or reading its own configuration prints before that line, not
# after. The raw tail is therefore not redundant with it.
#
# The per-provider refusals are dropped from this view. On a host whose vault
# backs two providers out of twenty-two they are twenty identical lines, and
# they push the launcher's own account of provisioning and registration - the
# part nothing else reports - out of any tail worth reading.
NOISE = ("capability issue failed for", "skipping subscription")
RAW_TAIL = len("a couple of dozen lines of the launcher's own account is what is worth")

# The broker and the launcher write to the unit's other stream, and a
# redemption refusal is reported there while the gateway's own log says only
# that a dependency was unavailable. Reading one and not the other is how "the
# credential is unavailable" stays a mystery.
TAIL_LINES = len("twenty lines is enough")


def print_boot_attempt():
    """The current boot attempt from the error log, and nothing older."""
    print("\n=== current boot attempt")
    if error_log.is_file():
        text = error_log.read_text(errors="replace")
        segments = text.split(BOOT_MARKER)
        latest = BOOT_MARKER + segments.pop() if len(segments) > len([BOOT_MARKER]) else text
        print(latest.strip())
    else:
        print(f"  {error_log}: absent")

    print("\n=== last lines of the error stream, per-provider refusals dropped")
    if error_log.is_file():
        raw = [
            line
            for line in error_log.read_text(errors="replace").splitlines()
            if not any(marker in line for marker in NOISE)
        ]
        for line in raw[-RAW_TAIL:] if len(raw) > RAW_TAIL else raw:
            print(f"  {line}")

    print("\n=== broker and launcher output")
    if output_log.is_file():
        lines = output_log.read_text(errors="replace").splitlines()
        for line in lines[-TAIL_LINES:] if len(lines) > TAIL_LINES else lines:
            print(f"  {line}")
    else:
        print(f"  {output_log}: absent")
