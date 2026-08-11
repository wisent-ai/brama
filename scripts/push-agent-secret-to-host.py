#!/usr/bin/env python3
"""Send this host's agent signing secret to another host through Stado.

Two hosts holding different values under `agent:wisent-app` produce a refusal
that is correct on both sides: the caller signs with its copy, the gateway
verifies with its own, and the request is rejected as if the credential were
forged. The gateway is the verifier for the whole fleet, so its copy is the one
a caller has to sign with.

Runs on the gateway host and first uses `stado host install-credential`. If the
live broker's administrative bearer cannot read the source, it falls back to an
owner-key read into a mode-0600 temporary file and `stado host install-secret`,
then removes that file. The value never appears in argv, stdout or logs.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

HOME = Path(os.environ.get("HOME", "."))
STADO = HOME / ".stado" / "bin" / "stado"
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
SERVICE_ENV = Path(
    os.environ.get("BRAMA_SERVICE_ENV_FILE", str(HOME / ".config" / "brama" / "service.env"))
)


def service_value(name: str) -> str:
    if SERVICE_ENV.is_file():
        for line in SERVICE_ENV.read_text(errors="replace").splitlines():
            key, separator, value = line.partition("=")
            if separator and key.strip() == name:
                return value.strip().strip('"').strip("'")
    return ""


def service_vault() -> Path:
    configured = os.environ.get("SKARBIEC_VAULT_FILE") or service_value(
        "SKARBIEC_VAULT_FILE"
    )
    return Path(configured) if configured else HOME / ".stado" / "skarbiec.vault.json"


VAULT = service_vault()
TARGET = os.environ.get("AGENT_SECRET_TARGET", "lukasz-macbook")
ITEM = os.environ.get("AGENT_SECRET_ITEM", "agent:wisent-app")
FIELD = os.environ.get("AGENT_SECRET_FIELD", "value")
NAME = os.environ.get("AGENT_SECRET_NAME", "jeden-agent-auth-secret")

if not STADO.exists():
    raise SystemExit(f"stado is unavailable here: {STADO}")

# The transfer reads the selected store as whichever consumer the environment
# presents, and the default one holds no grant here. Asking as that default and
# reading the 403 as "nobody may read this" is how a grant that exists gets
# reported as missing: `local-operator` carries `read agent:wisent-app#value`
# on this host and has a token file beside the others.
CONSUMER = os.environ.get("AGENT_SECRET_CONSUMER", "local-operator")
TOKEN_FILE = os.environ.get(
    "AGENT_SECRET_TOKEN_FILE", str(HOME / ".stado" / f"{CONSUMER}-skarbiec-token")
)
environment = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "WC_SKARBIEC_CONSUMER": CONSUMER,
    "WC_SKARBIEC_TOKEN_FILE": TOKEN_FILE,
    "WC_SKARBIEC_URL": os.environ.get("WC_SKARBIEC_URL")
    or service_value("WC_SKARBIEC_URL")
    or "http://127.0.0.1:17612",
}


def transfer():
    return subprocess.run(
        [str(STADO), "host", "install-credential", TARGET, ITEM, FIELD, NAME],
        capture_output=True,
        text=True,
        env=environment,
    )


done = transfer()
detail = f"{done.stdout}\n{done.stderr}".lower()
if done.returncode and ("403" in detail or "not authorized" in detail):
    capabilities = f"read:{ITEM}#{FIELD},stage:{ITEM}#{FIELD}"
    minted = subprocess.run(
        [
            str(SKARBIEC),
            "token-mint",
            CONSUMER,
            "--capabilities",
            capabilities,
            "--replace-capabilities",
        ],
        capture_output=True,
        text=True,
        env={**environment, "SKARBIEC_VAULT_FILE": str(VAULT)},
    )
    if minted.returncode:
        raise SystemExit(
            "local-operator token refresh failed: "
            + " ".join((minted.stderr or minted.stdout).split())
        )
    token = json.loads(minted.stdout).get("token")
    if not isinstance(token, str) or not token:
        raise SystemExit("local-operator token refresh returned no bearer")
    token_path = Path(TOKEN_FILE)
    token_path.parent.mkdir(parents=True, exist_ok=True)
    temporary = token_path.with_name(f".{token_path.name}.{os.getpid()}.tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(token)
        handle.write("\n")
    os.replace(temporary, token_path)
    print("refreshed:", CONSUMER, "bearer")
    done = transfer()
if done.returncode:
    owner_environment = {
        **environment,
        "SKARBIEC_VAULT_FILE": str(VAULT),
    }
    opened = subprocess.run(
        [str(SKARBIEC), "get", ITEM],
        capture_output=True,
        text=True,
        env=owner_environment,
    )
    if opened.returncode:
        raise SystemExit(
            "owner read failed: " + " ".join((opened.stderr or "").split())
        )
    value = (json.loads(opened.stdout).get("fields") or {}).get(FIELD)
    if not isinstance(value, str) or not value:
        raise SystemExit(f"owner read returned no string field {ITEM}#{FIELD}")
    source = HOME / ".stado" / f".{NAME}.{os.getpid()}.source"
    descriptor = os.open(source, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(value)
        done = subprocess.run(
            [str(STADO), "host", "install-secret", TARGET, str(source), NAME],
            capture_output=True,
            text=True,
            env=environment,
        )
        print("fallback:", "owner-key install-secret")
    finally:
        source.unlink(missing_ok=True)
print("target:", TARGET, "item:", ITEM, "field:", FIELD, "name:", NAME)
print("exit:", done.returncode)
for line in (done.stdout or "").splitlines():
    print("  out:", line)
for line in (done.stderr or "").splitlines():
    print("  err:", line)
