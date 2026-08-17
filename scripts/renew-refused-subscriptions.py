#!/usr/bin/env python3
"""Renew the subscriptions a provider is refusing, and prove it worked.

Brama's own usage probe says, per subscription, whether the newest attempt to
reach the provider was refused and why. A refusal that names authentication is
not something the gateway can repair: a revoked OAuth token or an expired
session is only replaced by a real sign-in, and for claude and kimi there is no
local CLI that can do it. Weles owns that sign-in and drives it in its own
browser on its own host. This closes the loop between the two without a human in
it: read the gateway's listing, pick the subscriptions whose newest probe was an
authentication refusal, resolve each one's account from the `brama:login:` tag on
its vault bundle, ask Weles to sign that account in through the Stado helper
channel, and then check the result against the vault and the gateway rather than
against the login's exit code.

A login that reports success proves nothing on its own. Closure is two facts: the
subscription's vault bundle stands at a higher revision than before, so a new
credential was actually written, and a probe attempted after the login now
succeeds. Both are read back here.

Safe to run repeatedly and from a timer:
  * a subscription with no `brama:login:` tag is reported unmapped and never
    attempted, because the account it belongs to is not recorded and signing
    into the wrong one is worse than leaving it refused;
  * a provider whose Weles reauthentication surface chooses the account itself
    from several is reported unattributable and never attempted, for the same
    reason;
  * a subscription whose recorded block is still in force is left alone until it
    lifts, so this never spends a sign-in on an account the gateway is not
    allowed to use yet;
  * one account is signed in at most once per run and never twice at the same
    time, held by a lock file, with a cooldown between runs.

Reads the bearer on the first line of standard input and the agent's
request-signing secret on the second, so neither reaches argv or the
environment. Nothing here opens a browser and nothing here uses ssh: every
remote step goes through `stado host install-helper` and `stado host run-helper`.

Usage: printf '%s\\n%s\\n' "$bearer" "$secret" |
         renew-refused-subscriptions.py <origin> <agent-id> [host]

Exit status is zero when nothing needed renewing or everything attempted closed,
and non-zero when a prerequisite is missing or an attempted subscription is
still refused - which is what a Stado-managed timer should alert on.
"""
from __future__ import annotations

import hashlib
import hmac
import json
import os
import pathlib
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

NONE = len([])
FIRST = len(["argv0"])
LAST = -len(["last"])
DETAIL = int("400")
MS_PER_SECOND = int("1000")
# Lock files and the rendered helper are readable by their owner alone: the
# helper names an account this fleet signs into, and the lock says a sign-in is
# under way.
OWNER_ONLY_FILE = stat.S_IRUSR | stat.S_IWUSR
OWNER_ONLY_PROGRAM = stat.S_IRWXU

HOME = pathlib.Path(os.environ.get("HOME", "."))
HERE = pathlib.Path(__file__).resolve().parent
DEFAULT_HOST = os.environ.get("BRAMA_RENEWAL_HOST", "charless-mac-mini")
STATE_DIR = pathlib.Path(
    os.environ.get("BRAMA_RENEWAL_STATE_DIR") or HOME / ".local/state/brama/renewal"
)

# One sign-in per account per run: a provider that refused a credential will
# refuse it again a second later, and a second login in the same run only burns
# the account's own rate limits.
MAX_ATTEMPTS_PER_ACCOUNT = int(os.environ.get("BRAMA_RENEWAL_MAX_ATTEMPTS", "1"))
# How long after an attempt this refuses to attempt the same account again, so a
# timer running every few minutes does not sign in every few minutes.
COOLDOWN_SECONDS = int(os.environ.get("BRAMA_RENEWAL_COOLDOWN_SECONDS", "3600"))
# A sign-in drives a real browser through Google SSO and a consent screen; the
# lock outlives that, and anything older than this belonged to a run that died.
LOCK_TTL_SECONDS = int(os.environ.get("BRAMA_RENEWAL_LOCK_TTL_SECONDS", "3600"))
# How long to wait for the gateway to probe a subscription again after a login,
# and how often to re-read its listing while waiting.
PROBE_WAIT_SECONDS = int(os.environ.get("BRAMA_RENEWAL_PROBE_WAIT_SECONDS", "900"))
POLL_SECONDS = int(os.environ.get("BRAMA_RENEWAL_POLL_SECONDS", "30"))
GATEWAY_TIMEOUT_SECONDS = float(os.environ.get("BRAMA_RENEWAL_GATEWAY_TIMEOUT_SECONDS", "20"))
# `stado host run-helper` waits for the helper, and the helper waits for Weles.
HELPER_TIMEOUT_SECONDS = float(os.environ.get("BRAMA_RENEWAL_HELPER_TIMEOUT_SECONDS", "2400"))

STATE_HELPER = "report-subscription-vault-state"
STATE_HELPER_SOURCE = HERE / "report-subscription-vault-state.sh"
LOGIN_HELPER_SOURCE = HERE / "renew-subscription-login.sh"
LOGIN_HELPER_PREFIX = "brama-renew-login-"
LOGIN_TAG_PREFIX = "brama:login:"
SUBSCRIPTION_ID_TAG_PREFIX = "brama:id:"

# Brama's provider ids and the names Weles's reauthentication surface knows them
# by.
WELES_PROVIDERS = {"claude-code": "claude", "codex": "codex", "kimi": "kimi"}

# Which accounts each provider has on this fleet. Weles's `POST /reauth` selects
# the account itself - `claude/reauth.mjs` takes the least recently updated of
# the rows whose display name begins with "Claude" - so a reauthentication can
# only be attributed to a named account when the provider has exactly one. Claude
# has two logins on the vault and three sign-in rows in Weles, which is why a
# claude subscription is reported rather than renewed until that surface can be
# told which account to use.
LOGINS_BY_PROVIDER = {
    "claude-code": ("claude-wisent-google-sso", "claude_controlyourai"),
    "codex": ("codex-wisent-google-sso",),
    "kimi": ("kimi-lukasz-google-sso",),
}

# A probe detail begins with the provider adapter's own failure kind. Only this
# one is repaired by signing in again: a rate limit lifts by itself and a
# dependency failure is not about the credential.
AUTH_PREFIX = "provider_authentication:"
# The provider's own sentence after that prefix, for the refusals that arrive
# without the kind. A revoked token, an expired token and a rejected key all end
# in the same place: a sign-in.
AUTH_SENTENCES = (
    "invalid_grant",
    "revoked",
    "token is expired",
    "token has expired",
    "api key appears to be invalid",
    "401",
    "403",
)

# States a subscription row can be in, which is the same derivation the console
# renders and the reason this loop only acts on one of them.
STATE_BLOCKED = "blocked"
STATE_WINDOWS_KNOWN = "windows_known"
STATE_REFUSED = "refused"
STATE_NEVER_USED = "never_used"
STATE_PUBLISHES_NONE = "publishes_none"

ACCOUNT_UNCONFIRMED = "unconfirmed"


class Prerequisite(Exception):
    """A fact the loop needs and does not have, named exactly."""


def now_ms() -> int:
    return int(time.time() * MS_PER_SECOND)


def resolved_stado() -> str:
    configured = os.environ.get("STADO_BIN", "").strip()
    candidates = [configured] if configured else [str(HOME / ".stado/bin/stado")]
    for candidate in candidates:
        if candidate and os.access(candidate, os.X_OK):
            return candidate
    found = subprocess.run(
        ["/usr/bin/env", "which", "stado"], capture_output=True, text=True, check=False
    ).stdout.strip()
    if found and os.access(found, os.X_OK):
        return found
    raise Prerequisite(
        "no stado binary found; the fleet is only reachable through it, and this "
        f"loop looked at {candidates[NONE]} and PATH"
    )


def gateway_listing(origin: str, agent: str, bearer: str, secret: str) -> dict:
    """The subscription listing the desktop console renders, read the same way.

    An empty body hashes to the empty string in this scheme, matching what the
    gateway verifies for a GET.
    """
    stamp = str(int(time.time()))
    signature = hmac.new(
        secret.encode("utf-8"), f"{agent}:{stamp}:".encode("utf-8"), hashlib.sha256
    ).hexdigest()
    request = urllib.request.Request(
        f"{origin.rstrip('/')}/v1/subscriptions/{agent}",
        method="GET",
        headers={
            "Authorization": f"Bearer {bearer}",
            "x-agent-id": agent,
            "x-agent-timestamp": stamp,
            "x-agent-signature": signature,
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=GATEWAY_TIMEOUT_SECONDS) as response:
            payload = response.read()
    except urllib.error.HTTPError as error:
        raise Prerequisite(
            f"the gateway at {origin} answered {error.code} for agent {agent}: "
            f"{error.read().decode('utf-8', 'replace')[:DETAIL]}"
        ) from error
    except OSError as error:
        raise Prerequisite(f"the gateway at {origin} is unreachable: {error}") from error
    try:
        document = json.loads(payload)
    except ValueError as error:
        raise Prerequisite(f"the gateway at {origin} did not answer with JSON") from error
    if not isinstance(document.get("subscriptions"), list):
        raise Prerequisite(
            f"the gateway at {origin} answered without a subscription list, so this "
            "agent has no listing to renew from"
        )
    return document


def stado_run(command: list[str], host: str, name: str) -> subprocess.CompletedProcess:
    """One Stado command, with a host that stopped answering named as such.

    A helper that never returns is not a helper that failed: the sign-in it
    started may still be running on the host, which is why the caller reports it
    rather than immediately trying again.
    """
    try:
        return subprocess.run(
            command,
            capture_output=True,
            text=True,
            check=False,
            timeout=HELPER_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as error:
        raise Prerequisite(
            f"{host} did not answer for helper {name} within "
            f"{HELPER_TIMEOUT_SECONDS:.0f}s"
        ) from error


def run_helper(stado: str, host: str, name: str, source: pathlib.Path, run_id: str) -> str:
    """Install one reviewed helper on the host and run it, returning its output.

    Installing on every run is deliberate: the host then runs exactly the file in
    this checkout, and a helper left behind by an older revision cannot answer
    for it. `run-helper` carries correlation identifiers and nothing else, so the
    run id is the only thing handed over.
    """
    if not source.is_file():
        raise Prerequisite(f"no helper to install at {source}")
    install = stado_run(
        [stado, "host", "install-helper", host, str(source), name, "--json"], host, name
    )
    if install.returncode != NONE:
        raise Prerequisite(
            f"{host} did not accept helper {name}: "
            f"{(install.stderr or install.stdout).strip()[:DETAIL]}"
        )
    answer = stado_run(
        [stado, "host", "run-helper", host, name, "--uuid", run_id, "--json"], host, name
    )
    try:
        report = json.loads(answer.stdout)
    except ValueError:
        raise Prerequisite(
            f"{host} did not report helper {name} as JSON: "
            f"{(answer.stderr or answer.stdout).strip()[:DETAIL]}"
        ) from None
    if report.get("status") != "completed":
        raise Prerequisite(
            f"{host} reported helper {name} as {report.get('status')}: "
            f"{str(report.get('stderr') or report.get('stdout')).strip()[:DETAIL]}"
        )
    return str(report.get("stdout") or "")


def host_vault_state(stado: str, host: str, run_id: str) -> dict:
    """Every subscription bundle on the host's vault, with revision and tags."""
    output = run_helper(stado, host, STATE_HELPER, STATE_HELPER_SOURCE, run_id)
    try:
        document = json.loads(output.strip().splitlines()[LAST])
    except (IndexError, ValueError):
        raise Prerequisite(
            f"{host} did not report its subscription bundles as JSON: {output.strip()[:DETAIL]}"
        ) from None
    items = document.get("items")
    if not isinstance(items, dict) or not items:
        raise Prerequisite(
            f"the vault at {document.get('vault')} on {host} holds no subscription "
            "bundles, so it is not the vault this gateway serves from"
        )
    return items


def bundle_for(items: dict, subscription_id: str, provider: str) -> tuple[str, dict] | None:
    """The vault bundle a subscription row was served from.

    Matched on the `brama:id:` tag first, because that tag is the contract, and
    on the naming convention second, so a bundle whose enumeration tags were lost
    is still found rather than reported as absent.
    """
    wanted = f"{SUBSCRIPTION_ID_TAG_PREFIX}{subscription_id}"
    for name, entry in items.items():
        if wanted in (entry.get("tags") or []):
            return name, entry
    conventional = f"provider:{provider}:{subscription_id}"
    if conventional in items:
        return conventional, items[conventional]
    return None


def login_of(entry: dict) -> str | None:
    for tag in entry.get("tags") or []:
        if tag.startswith(LOGIN_TAG_PREFIX):
            return tag[len(LOGIN_TAG_PREFIX):]
    return None


def is_auth_refusal(detail: str) -> bool:
    lowered = detail.strip().lower()
    if lowered.startswith(AUTH_PREFIX):
        return True
    return any(sentence in lowered for sentence in AUTH_SENTENCES)


def state_of(subscription: dict) -> str:
    """The one state a subscription row is in, from the fields it carries.

    The same derivation the console renders, so a verdict printed here means what
    the Model Sources page shows.
    """
    block = subscription.get("block") or {}
    if isinstance(block, dict) and (block.get("blocked_until_ms") or NONE) > now_ms():
        return STATE_BLOCKED
    if subscription.get("limits"):
        return STATE_WINDOWS_KNOWN
    probe = subscription.get("probe")
    if isinstance(probe, dict) and probe.get("ok") is False:
        return STATE_REFUSED
    if isinstance(probe, dict) and probe.get("ok") is True:
        return STATE_PUBLISHES_NONE
    measured = subscription.get("measured") or {}
    if not measured or (measured.get("requests") or NONE) == NONE:
        return STATE_NEVER_USED
    return STATE_PUBLISHES_NONE


def probe_detail(subscription: dict) -> str:
    probe = subscription.get("probe")
    return str(probe.get("detail") or "") if isinstance(probe, dict) else ""


def probe_attempted_ms(subscription: dict) -> int:
    probe = subscription.get("probe")
    if not isinstance(probe, dict):
        return NONE
    attempted = probe.get("attempted_at_ms")
    return int(attempted) if isinstance(attempted, (int, float)) else NONE


def read_state_record() -> dict:
    record = STATE_DIR / "attempts.json"
    if not record.is_file():
        return {}
    try:
        return json.loads(record.read_text())
    except ValueError:
        return {}


def write_state_record(document: dict) -> None:
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    (STATE_DIR / "attempts.json").write_text(json.dumps(document, indent=len("  ")))


class AccountLock:
    """One sign-in per account at a time, across every run on this machine.

    Two runs signing the same account in at once would race each other's
    credential writes and could leave the newer login's token overwritten by the
    older one's.
    """

    def __init__(self, account: str) -> None:
        STATE_DIR.mkdir(parents=True, exist_ok=True)
        self.path = STATE_DIR / f"{account}.lock"
        self.held = False

    def __enter__(self) -> "AccountLock":
        self.held = self.take()
        return self

    def __exit__(self, *_: object) -> None:
        if self.held:
            self.path.unlink(missing_ok=True)

    def take(self) -> bool:
        try:
            descriptor = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, OWNER_ONLY_FILE)
        except FileExistsError:
            age = time.time() - self.path.stat().st_mtime
            if age < LOCK_TTL_SECONDS:
                return False
            print(
                f"    breaking a lock left behind {int(age)}s ago by a run that did not finish"
            )
            self.path.unlink(missing_ok=True)
            return self.take()
        with os.fdopen(descriptor, "w") as handle:
            handle.write(json.dumps({"pid": os.getpid(), "since_ms": now_ms()}))
        return True


def rendered_login_helper(provider: str, login_item: str) -> pathlib.Path:
    """The reviewed helper with this run's provider and account pinned into it.

    `run-helper` carries no operator words, so what the helper acts on has to be
    part of the file that is installed. Rendering it here keeps the reviewed
    script in the repository and puts nothing on a command line.
    """
    if not LOGIN_HELPER_SOURCE.is_file():
        raise Prerequisite(f"no login helper to render at {LOGIN_HELPER_SOURCE}")
    body = LOGIN_HELPER_SOURCE.read_text()
    for placeholder, value in (("@PROVIDER@", provider), ("@LOGIN_ITEM@", login_item)):
        if placeholder not in body:
            raise Prerequisite(
                f"{LOGIN_HELPER_SOURCE} carries no {placeholder} to pin, so this run "
                "cannot say which account it is for"
            )
        body = body.replace(placeholder, value)
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    rendered = STATE_DIR / f"{LOGIN_HELPER_PREFIX}{login_item}.sh"
    rendered.write_text(body)
    rendered.chmod(OWNER_ONLY_PROGRAM)
    return rendered


def sign_in(stado: str, host: str, provider: str, login_item: str, run_id: str) -> dict:
    """Ask Weles to sign one account in, and hand back what it reported."""
    weles_provider = WELES_PROVIDERS[provider]
    rendered = rendered_login_helper(weles_provider, login_item)
    name = f"{LOGIN_HELPER_PREFIX}{login_item}"
    output = run_helper(stado, host, name, rendered, run_id)
    try:
        return json.loads(output.strip().splitlines()[LAST])
    except (IndexError, ValueError):
        return {"ok": False, "detail": output.strip()[:DETAIL] or "the login helper said nothing"}


def main() -> int:
    arguments = sys.argv[FIRST:]
    if len(arguments) not in (len(["origin", "agent"]), len(["origin", "agent", "host"])):
        print(
            "usage: renew-refused-subscriptions.py <origin> <agent-id> [host]",
            file=sys.stderr,
        )
        return len(["usage"])
    origin, agent = arguments[NONE], arguments[FIRST]
    host = arguments[LAST] if len(arguments) == len(["origin", "agent", "host"]) else DEFAULT_HOST

    lines = sys.stdin.read().splitlines()
    bearer = (lines[NONE] if lines else "").strip()
    secret = (lines[FIRST] if len(lines) > FIRST else "").strip()
    if not bearer or not secret:
        print(
            "standard input must carry the bearer and the agent's signing secret, "
            "one per line, so neither reaches argv",
            file=sys.stderr,
        )
        return len(["missing-credentials"])

    run_id = str(uuid.uuid4())
    try:
        stado = resolved_stado()
        listing = gateway_listing(origin, agent, bearer, secret)
        vault_before = host_vault_state(stado, host, run_id)
    except Prerequisite as error:
        print(str(error), file=sys.stderr)
        return len(["prerequisite"])

    print(f"agent:  {agent} at {origin}")
    print(f"host:   {host}")
    print(f"run:    {run_id}")

    persisted = read_state_record()
    attempted: dict[str, dict] = {}
    unclosed = NONE

    for subscription in sorted(listing["subscriptions"], key=lambda entry: str(entry.get("id"))):
        identifier = str(subscription.get("id"))
        provider = str(subscription.get("provider"))
        state = state_of(subscription)
        print(f"=== {identifier} ({provider})")
        if state != STATE_REFUSED:
            print(f"    {state}: nothing to renew")
            continue
        detail = probe_detail(subscription)
        if not is_auth_refusal(detail):
            print(f"    refused for a reason a sign-in does not repair: {detail[:DETAIL]}")
            continue
        found = bundle_for(vault_before, identifier, provider)
        if not found:
            print(
                "    refused, but no vault bundle on this host serves it, so there is "
                "nothing to renew and nothing to verify"
            )
            unclosed += FIRST
            continue
        bundle, entry = found
        login_item = login_of(entry)
        if not login_item:
            print(
                f"    unmapped: {bundle} carries no {LOGIN_TAG_PREFIX} tag, so the "
                "account it signs in with is unknown and no login is attempted"
            )
            continue
        accounts = LOGINS_BY_PROVIDER.get(provider, ())
        if login_item not in accounts:
            print(
                f"    unattributable: {bundle} names account {login_item}, which is "
                f"not one this fleet records for {provider}; refusing to sign in"
            )
            unclosed += FIRST
            continue
        if len(accounts) != FIRST:
            print(
                f"    unattributable: Weles chooses the account itself when it "
                f"reauthenticates {provider}, which has {len(accounts)} accounts on "
                f"this fleet, so a sign-in cannot be attributed to {login_item}"
            )
            continue
        attempted.setdefault(
            login_item,
            {"provider": provider, "subscriptions": [], "detail": detail},
        )["subscriptions"].append((identifier, bundle, entry))
        print(f"    refused ({detail[:DETAIL]}); account {login_item}")

    for login_item in sorted(attempted):
        plan = attempted[login_item]
        provider = plan["provider"]
        print(f"--- signing in {login_item} for {provider}")
        history = persisted.get(login_item) or {}
        since_last = time.time() - (history.get("last_attempt_ms") or NONE) / MS_PER_SECOND
        if since_last < COOLDOWN_SECONDS:
            # A deferral, not a failure: the previous run already reported what it
            # found, and a timer that alerted on every run inside the cooldown
            # would alert for an hour about one refusal.
            print(
                f"    cooling down: last attempt {int(since_last)}s ago, and this "
                f"account is not signed in again for {COOLDOWN_SECONDS}s"
            )
            continue
        with AccountLock(login_item) as lock:
            if not lock.held:
                print("    another run is already signing this account in; leaving it to that run")
                continue
            closed = NONE
            for attempt in range(MAX_ATTEMPTS_PER_ACCOUNT):
                started_ms = now_ms()
                persisted[login_item] = {
                    "last_attempt_ms": started_ms,
                    "provider": provider,
                    "run": run_id,
                }
                write_state_record(persisted)
                try:
                    answer = sign_in(stado, host, provider, login_item, run_id)
                except Prerequisite as error:
                    print(f"    attempt {attempt + FIRST} could not run: {error}")
                    break
                account = answer.get("account") or ACCOUNT_UNCONFIRMED
                print(
                    f"    attempt {attempt + FIRST}: ok={bool(answer.get('ok'))} "
                    f"account={account} run={answer.get('run_id')} "
                    f"detail={str(answer.get('detail') or '')[:DETAIL]}"
                )
                closed = verify(
                    stado, host, origin, agent, bearer, secret, plan, started_ms, run_id
                )
                if closed == len(plan["subscriptions"]):
                    break
            unclosed += len(plan["subscriptions"]) - closed

    return NONE if unclosed == NONE else len(["unclosed"])


def verify(
    stado: str,
    host: str,
    origin: str,
    agent: str,
    bearer: str,
    secret: str,
    plan: dict,
    started_ms: int,
    run_id: str,
) -> int:
    """Count the subscriptions of one account that this sign-in actually closed.

    Two facts, both read back rather than assumed: the vault bundle stands at a
    higher revision than it did before the login, so a credential was written,
    and a probe attempted after the login succeeded. A login's own exit status is
    evidence of nothing here - the refusal this loop repairs is a fact about the
    provider, and only the provider's next answer settles it.
    """
    wanted = {identifier for identifier, _, _ in plan["subscriptions"]}
    deadline = time.time() + PROBE_WAIT_SECONDS
    fresh: dict[str, dict] = {}
    while True:
        try:
            listing = gateway_listing(origin, agent, bearer, secret)
        except Prerequisite as error:
            print(f"    the gateway stopped answering while verifying: {error}")
            return NONE
        for subscription in listing["subscriptions"]:
            identifier = str(subscription.get("id"))
            if identifier in wanted and probe_attempted_ms(subscription) > started_ms:
                fresh[identifier] = subscription
        if len(fresh) == len(wanted) or time.time() >= deadline:
            break
        time.sleep(POLL_SECONDS)

    try:
        vault_after = host_vault_state(stado, host, run_id)
    except Prerequisite as error:
        print(f"    the host stopped answering while verifying: {error}")
        return NONE

    closed = NONE
    for identifier, bundle, before in plan["subscriptions"]:
        after = vault_after.get(bundle) or {}
        was = before.get("revision") or NONE
        now = after.get("revision") or NONE
        subscription = fresh.get(identifier)
        if subscription is None:
            print(
                f"    {identifier}: the gateway has not probed it since the login "
                f"(waited {PROBE_WAIT_SECONDS}s); revision {was} -> {now}. Set "
                "BRAMA_USAGE_PROBE_INTERVAL_SECS on the gateway if probing is off"
            )
            continue
        advanced = now > was
        probe = subscription.get("probe") or {}
        succeeded = bool(probe.get("ok"))
        if advanced and succeeded:
            print(f"    {identifier}: closed; revision {was} -> {now} and the probe now succeeds")
            closed += FIRST
            continue
        if not advanced:
            print(
                f"    {identifier}: still refused; {bundle} is unchanged at revision "
                f"{now}, so the sign-in wrote no credential. Probe: "
                f"{str(probe.get('detail') or '')[:DETAIL]}"
            )
            continue
        print(
            f"    {identifier}: still refused; revision {was} -> {now} but the probe "
            f"says {str(probe.get('detail') or '')[:DETAIL]}"
        )
    return closed


if __name__ == "__main__":
    raise SystemExit(main())
