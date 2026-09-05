#!/usr/bin/env python3
"""Say whether this host can verify an Ed25519 capability proof at all.

The broker verifies a workload's proof by shelling out to `openssl pkeyutl`.
Apple ships LibreSSL under that name and LibreSSL implements no Ed25519, so on a
stock Mac every proof fails and the answer is `capability redemption denied` —
the same message a wrong key, an expired grant and a spent capability produce.

This checks each candidate the broker would pick, in its order, and reports which
one it lands on and whether that build can actually verify. The check is a real
verification: a key pair is generated, a signature made, and the candidate asked
to confirm it.

Read-only with respect to the installation; it writes only to a temporary
directory.
"""

import os
import pathlib
import subprocess
import tempfile

CANDIDATES = (
    "/opt/homebrew/opt/openssl@3/bin/openssl",
    "/opt/homebrew/bin/openssl",
    "/usr/local/opt/openssl@3/bin/openssl",
    "openssl",
)

configured = os.environ.get("SKARBIEC_OPENSSL", "")
order = ([configured] if configured else []) + list(CANDIDATES)
chosen = next(
    (name for name in order if name == "openssl" or pathlib.Path(name).exists()), None
)
print(f"SKARBIEC_OPENSSL: {configured or 'unset'}")
print(f"broker would use: {chosen}")

with tempfile.TemporaryDirectory() as workspace:
    work = pathlib.Path(workspace)
    private = work / "private.pem"
    public = work / "public.pem"
    payload = work / "payload.bin"
    signature = work / "payload.sig"
    payload.write_bytes(b"skarbiec capability proof probe")

    for candidate in order:
        if candidate != "openssl" and not pathlib.Path(candidate).exists():
            print(f"{candidate}: absent")
            continue
        version = subprocess.run(
            [candidate, "version"], capture_output=True, text=True, check=False
        )
        banner = version.stdout.strip() or version.stderr.strip()
        generated = subprocess.run(
            [candidate, "genpkey", "-algorithm", "ed25519", "-out", str(private)],
            capture_output=True,
            text=True,
            check=False,
        )
        if generated.returncode:
            print(f"{candidate}: {banner} — no Ed25519 ({generated.stderr.strip()})")
            continue
        subprocess.run(
            [candidate, "pkey", "-in", str(private), "-pubout", "-out", str(public)],
            capture_output=True,
            check=False,
        )
        subprocess.run(
            [
                candidate, "pkeyutl", "-sign", "-inkey", str(private),
                "-rawin", "-in", str(payload), "-out", str(signature),
            ],
            capture_output=True,
            check=False,
        )
        verified = subprocess.run(
            [
                candidate, "pkeyutl", "-verify", "-pubin", "-inkey", str(public),
                "-rawin", "-in", str(payload), "-sigfile", str(signature),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        outcome = "verifies Ed25519" if verified.returncode == int() else "cannot verify"
        print(f"{candidate}: {banner} — {outcome}")
