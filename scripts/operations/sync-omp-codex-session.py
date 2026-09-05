#!/usr/bin/env python3
"""Keep an existing OMP Codex session in its exact Brama-owned Skarbiec item.

OMP remains the only refresh-token owner. Brama receives only the access token
through its signed donation API; Stado confirms the vault item and consumers.
No browser or sign-in is started. --watch repeats this synchronization.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request


class Refusal(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def command_value(arguments: list[str], environment: dict[str, str]) -> str:
    result = subprocess.run(
        arguments, env=environment, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=60,
    )
    if result.returncode:
        # A credential command can include its input in an error. Keep its
        # output within this credential-use boundary, including on failure.
        raise Refusal(f"{Path(arguments[0]).name} {arguments[1]} failed with exit {result.returncode}")
    value = result.stdout.strip()
    if not value:
        raise Refusal(f"{Path(arguments[0]).name} returned an empty credential")
    return value


def access_claims(token: str, options: argparse.Namespace) -> dict:
    try:
        parts = token.split(".")
        if len(parts) != 3:
            raise ValueError()
        payload = parts[1]
        claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        email = claims["https://api.openai.com/profile"]["email"]
        account_id = claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        expires = claims["exp"]
        if not isinstance(expires, (int, float)) or isinstance(expires, bool):
            raise ValueError()
    except (ValueError, KeyError, TypeError):
        raise Refusal("OMP did not return a Codex access token with account and expiry claims") from None
    if email != options.email or account_id != options.account_id:
        raise Refusal("OMP account order or identity changed; refusing to donate another account")
    return claims


def current_access(options: argparse.Namespace, environment: dict[str, str]) -> tuple[str, int]:
    arguments = [options.omp, "token", "openai-codex", "--account", str(options.account), "--raw"]
    token = command_value(arguments, environment)
    claims = access_claims(token, options)
    # Refresh through OMP's credential store, not with a copied refresh token.
    # This preserves the same refresh owner used by existing OMP sessions.
    if claims["exp"] <= time.time() + 600:
        token = command_value(arguments + ["--force-refresh"], environment)
        claims = access_claims(token, options)
    if claims["exp"] <= time.time() + 600:
        raise Refusal("OMP did not supply an access token valid beyond the renewal window")
    return token, int(claims["exp"])


def subscription_request(
    options: argparse.Namespace, bearer: str, signing_secret: str,
    document: dict | None = None,
) -> dict:
    body = None if document is None else json.dumps(document, separators=(",", ":")).encode()
    body_hash = "" if body is None else hashlib.sha256(body).hexdigest()
    stamp = str(int(time.time()))
    signature = hmac.new(
        signing_secret.encode(),
        f"{options.agent_id}:{stamp}:{body_hash}".encode(),
        hashlib.sha256,
    ).hexdigest()
    request = urllib.request.Request(
        f"{options.brama_url.rstrip('/')}/v1/subscriptions/{options.agent_id}",
        data=body,
        headers={
            "Authorization": f"Bearer {bearer}",
            "Content-Type": "application/json",
            "x-agent-body-sha256": body_hash,
            "x-agent-id": options.agent_id,
            "x-agent-timestamp": stamp,
            "x-agent-signature": signature,
        },
    )
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=60) as response:
            payload = response.read(1024 * 1024 + 1)
    except urllib.error.HTTPError as error:
        detail = error.read(4096).decode("utf-8", errors="replace")
        raise Refusal(f"Brama subscription request refused with HTTP {error.code}: {detail}") from None
    if len(payload) > 1024 * 1024:
        raise Refusal("Brama subscription response exceeds its metadata size limit")
    return json.loads(payload)


def stado_command(options: argparse.Namespace, arguments: list[str]) -> dict:
    result = subprocess.run(
        [options.stado, "host", *arguments, "--json"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=180,
    )
    if result.returncode:
        raise Refusal(
            f"stado host {arguments[0]} exited {result.returncode}: {result.stderr.strip()}"
        )
    return json.loads(result.stdout)


def synchronize(options: argparse.Namespace) -> dict:
    environment = os.environ.copy()
    environment["SKARBIEC_VAULT_FILE"] = str(options.vault)
    bearer = command_value([options.skarbiec, "get", options.bearer_item, "--field", "token"], environment)
    signing_secret = command_value([options.skarbiec, "get", options.signing_item, "--field", "value"], environment)
    listing = subscription_request(options, bearer, signing_secret)
    existing = next(
        (item for item in listing["subscriptions"] if item["id"] == options.subscription_id), None,
    )
    if existing is None or existing["provider"] != "codex":
        raise Refusal("The signed Brama agent does not own the exact Codex subscription")
    if existing.get("login_item") not in {None, options.login_item}:
        raise Refusal(
            f"The subscription maps to {existing.get('login_item')!r}, not {options.login_item!r}"
        )

    token, expires = current_access(options, environment)
    credential = json.dumps({"tokens": {
        "access_token": token,
        "account_id": options.account_id,
    }, "expires_at": expires}, separators=(",", ":"))
    expected_digest = hashlib.sha256(credential.encode()).hexdigest()
    item_id = f"provider:codex:{options.subscription_id}"
    show = ["vault-item-show", options.host, item_id]
    before = stado_command(options, show)
    tags = before["tags"].split(",")
    for prefix, expected in (
        ("brama:provider:", "codex"),
        ("brama:id:", options.subscription_id),
    ):
        if [tag.removeprefix(prefix) for tag in tags if tag.startswith(prefix)] != [expected]:
            raise Refusal(f"{item_id} does not carry its exact {prefix}{expected} mapping")
    if f"brama:agent:{options.agent_id}" not in tags or "brama:subscription" not in tags:
        raise Refusal(f"{item_id} is not assigned to the signed Brama agent")
    login_tags = [tag for tag in tags if tag.startswith("brama:login:")]
    expected_login = f"brama:login:{options.login_item}" if options.login_item else None
    if login_tags and login_tags != [expected_login]:
        raise Refusal(f"{item_id} belongs to login mapping {login_tags!r}")
    if before.get("state") != "active" or before.get("kind") != "bundle":
        raise Refusal(f"{item_id} is not an active credential bundle")
    if [field["name"] for field in before["fields"]] != ["value"]:
        raise Refusal(f"{item_id} contains fields this session synchronization must not replace")

    changed = before["fields"][0]["sha256"] != expected_digest
    add_login_mapping = expected_login is not None and not login_tags
    if add_login_mapping:
        tags.append(expected_login)
    # Writing the vault alone does not acknowledge a replaced grant to Brama:
    # its request path still skips a recorded reauthorization block. Use the
    # signed donation operation so the credential and that record agree.
    needs_acknowledgment = (
        changed or add_login_mapping
        or (existing.get("credential") or {}).get("state") != "active"
    )
    if needs_acknowledgment:
        subscription_request(options, bearer, signing_secret, {
            "provider": "codex",
            "subscription_id": options.subscription_id,
            "login_item": options.login_item,
            "label": f"omp:{options.email}",
            "api_key": credential,
        })
    after = stado_command(options, show) if needs_acknowledgment else before
    if after["fields"][0]["sha256"] != expected_digest or set(after["tags"].split(",")) != set(tags):
        raise Refusal(f"{item_id} did not retain the access token and all existing consumer tags")
    return {
        "subscription": options.subscription_id, "account": options.email,
        "access_expires_at": expires, "result": "stored" if changed else "current",
        "revision": after["revision"], "host": options.host,
        "refresh_owner": "omp", "reason": options.reason,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--brama-url", required=True)
    parser.add_argument("--host", required=True, help="Stado-selected host owning Brama's vault")
    parser.add_argument("--stado", default=str(Path.home() / ".local/bin/stado"))
    parser.add_argument("--agent-id", required=True)
    parser.add_argument("--subscription-id", required=True)
    parser.add_argument("--login-item", help="Exact existing login mapping, if this subscription has one")
    parser.add_argument("--account", required=True, type=int, help="OMP's 1-based stored account index")
    parser.add_argument("--email", required=True)
    parser.add_argument("--account-id", required=True, help="Expected ChatGPT account UUID")
    parser.add_argument("--bearer-item", required=True)
    parser.add_argument("--signing-item", required=True)
    parser.add_argument("--reason", required=True)
    parser.add_argument("--omp", default=str(Path.home() / ".local/bin/omp"))
    parser.add_argument("--skarbiec", default=str(Path.home() / ".local/bin/skarbiec"))
    parser.add_argument("--vault", type=Path, default=Path.home() / ".stado/skarbiec.vault.json")
    parser.add_argument("--watch", action="store_true", help="Keep synchronizing every five minutes under Stado")
    options = parser.parse_args()
    origin = urllib.parse.urlsplit(options.brama_url)
    if (origin.scheme != "https" and not (
        origin.scheme == "http" and origin.hostname in {"127.0.0.1", "::1"}
    )) or not origin.hostname or origin.username or origin.password or origin.query or origin.fragment or origin.path not in {"", "/"}:
        parser.error("Brama must be an HTTPS origin or authenticated loopback origin")
    if not options.agent_id or any(character not in "abcdefghijklmnopqrstuvwxyz0123456789-" for character in options.agent_id):
        parser.error("agent-id must contain lowercase ASCII letters, digits or hyphens")
    if options.account < 1 or not options.reason.strip():
        parser.error("account must be positive and reason must be nonempty")
    stopped = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stopped.set())
    signal.signal(signal.SIGINT, lambda *_: stopped.set())
    while not stopped.is_set():
        try:
            print(json.dumps(synchronize(options)), flush=True)
        except (Refusal, subprocess.TimeoutExpired, OSError, ValueError, KeyError, TypeError) as error:
            detail = f"{type(error).__name__}: {error}"
            print(json.dumps({"result": "refused", "detail": detail, "reason": options.reason}), file=sys.stderr, flush=True)
            return 1
        if not options.watch or stopped.wait(300):
            break
    return 0


if __name__ == "__main__":
    sys.exit(main())
