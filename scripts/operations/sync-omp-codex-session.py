#!/usr/bin/env python3
"""Keep one existing OMP Codex account donated to its exact Brama subscription.

OMP remains the only owner of the refresh token. Only the current access token
crosses the authenticated Brama donation endpoint; no browser or sign-in is
started. Run with --watch under Stado to publish subsequent OMP refreshes.
Credentials are acquired through the owner Skarbiec CLI, never command arguments.
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


def gateway_request(options: argparse.Namespace, bearer: str, signing_secret: str, body: dict | None) -> dict:
    encoded = b"" if body is None else json.dumps(body, separators=(",", ":")).encode()
    stamp = str(int(time.time()))
    digest = hashlib.sha256(encoded).hexdigest() if encoded else ""
    signature = hmac.new(
        signing_secret.encode(), f"{options.agent_id}:{stamp}:{digest}".encode(), hashlib.sha256,
    ).hexdigest()
    request = urllib.request.Request(
        f"{options.brama_url.rstrip('/')}/v1/subscriptions/{options.agent_id}",
        data=encoded if body is not None else None,
        headers={
            "Authorization": f"Bearer {bearer}",
            "x-agent-id": options.agent_id,
            "x-agent-timestamp": stamp,
            "x-agent-signature": signature,
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=60) as response:
            payload = response.read(1024 * 1024 + 1)
    except urllib.error.HTTPError as error:
        raise Refusal(f"Brama subscription {'donation' if body is not None else 'listing'} refused with HTTP {error.code}") from None
    except urllib.error.URLError:
        raise Refusal("Brama subscription endpoint is unreachable") from None
    if len(payload) > 1024 * 1024:
        raise Refusal("Brama subscription response exceeds its metadata size limit")
    try:
        value = json.loads(payload)
    except ValueError:
        raise Refusal("Brama subscription response is not JSON") from None
    if not isinstance(value, dict):
        raise Refusal("Brama subscription response is not an object")
    return value


def synchronize(options: argparse.Namespace) -> dict:
    environment = os.environ.copy()
    environment["SKARBIEC_VAULT_FILE"] = str(options.vault)
    bearer = command_value([options.skarbiec, "get", options.bearer_item, "--field", "token"], environment)
    signing_secret = command_value([options.skarbiec, "get", options.signing_item, "--field", "value"], environment)
    listing = gateway_request(options, bearer, signing_secret, None)
    subscriptions = listing.get("subscriptions")
    if not isinstance(subscriptions, list):
        raise Refusal("Brama did not return its subscription list")
    existing = next((item for item in subscriptions if item.get("id") == options.subscription_id), None)
    if existing is None:
        raise Refusal("The exact subscription is not owned by this signed agent")
    if existing.get("provider") != "codex":
        raise Refusal("The exact subscription does not use Codex")
    if existing.get("login_item") != options.login_item:
        raise Refusal("The subscription's login item differs from the named account mapping")

    token, expires = current_access(options, environment)
    label = f"omp:{options.email}:{hashlib.sha256(token.encode()).hexdigest()}"
    changed = existing.get("label") != label
    if changed:
        credential = json.dumps({"tokens": {
            "access_token": token,
            "account_id": options.account_id,
        }, "expires_at": expires}, separators=(",", ":"))
        result = gateway_request(options, bearer, signing_secret, {
            "provider": "codex", "label": label, "api_key": credential,
            "login_item": options.login_item, "subscription_id": options.subscription_id,
        })
        if result.get("subscription", {}).get("id") != options.subscription_id:
            raise Refusal("Brama did not acknowledge the exact donated subscription")
        observed = gateway_request(options, bearer, signing_secret, None)
        if not any(item.get("id") == options.subscription_id and item.get("label") == label
                   for item in observed.get("subscriptions", [])):
            raise Refusal("Brama did not retain the donated subscription metadata")
    return {
        "subscription": options.subscription_id, "account": options.email,
        "access_expires_at": expires, "result": "donated" if changed else "current",
        "refresh_owner": "omp", "reason": options.reason,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--brama-url", required=True)
    parser.add_argument("--agent-id", required=True)
    parser.add_argument("--subscription-id", required=True)
    parser.add_argument("--login-item", required=True)
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
    if options.subscription_id != f"brama-sub-{options.agent_id}-codex-primary":
        parser.error("subscription-id must be the named agent's existing Codex primary")
    if options.account < 1 or not options.reason.strip():
        parser.error("account must be positive and reason must be nonempty")
    stopped = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stopped.set())
    signal.signal(signal.SIGINT, lambda *_: stopped.set())
    while not stopped.is_set():
        try:
            print(json.dumps(synchronize(options)), flush=True)
        except (Refusal, subprocess.TimeoutExpired, OSError) as error:
            detail = str(error) if isinstance(error, Refusal) else "credential synchronization command or transport failed"
            print(json.dumps({"result": "refused", "detail": detail, "reason": options.reason}), file=sys.stderr, flush=True)
            return 1
        if not options.watch or stopped.wait(300):
            break
    return 0


if __name__ == "__main__":
    sys.exit(main())
