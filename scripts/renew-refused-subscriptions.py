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

A bundle that records no account is learned rather than skipped. Which of several
subscriptions one login mints is written down nowhere and cannot be read out of
the vault - the account is only visible in what a sign-in writes - so when a
provider has exactly one account no bundle is attributed to, this snapshots every
unmapped bundle's revision, signs that account in once, and writes
`brama:login:<login-item>` on whichever bundles the sign-in actually advanced,
through Skarbiec's own mapping script so every tag already on them is preserved.
Two accounts nothing is attributed to means nothing is written: the observation
would not say which one was responsible.

Safe to run repeatedly and from a timer:
  * a subscription with no `brama:login:` tag is never signed in on a guess, and
    no longer left unrenewable either: the loop learns which account mints it by
    watching one sign-in of the single account of that provider no bundle is
    attributed to yet, and writes `brama:login:` on whichever bundles that
    sign-in actually advanced;
  * a provider with two or more accounts no bundle is attributed to is reported
    ambiguous, both named, and left unmapped, because a sign-in there would not
    say which of them minted a bundle;
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

A Stado-managed unit runs exactly that command on a schedule, with the two lines
of standard input coming from an owner-only file the unit reads and the knobs
below coming from its environment - BRAMA_RENEWAL_COOLDOWN_SECONDS is the one
that decides how often a refused account is actually signed in again, so a unit
may run far more often than it acts. No unit is installed by this file: what to
schedule and where is the fleet's decision, not this script's.
"""
from __future__ import annotations

import contextlib
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


# The helpers sit beside this file, and where that is decides their names. In the
# repository they carry their extension; in a release bundle every program is
# installed under its bare name, because that is the shape `stado service update`
# unpacks and the unit's program path expects. Resolving only the repository name
# made the loop exit at once inside the release with "no helper to install", and
# nothing on the schedule ever ran.
def beside(*names: str) -> pathlib.Path:
    for name in names:
        candidate = HERE / name
        if candidate.exists():
            return candidate
    return HERE / names[0]


STATE_HELPER_SOURCE = beside("report-subscription-vault-state.sh", STATE_HELPER)
LOGIN_HELPER_SOURCE = beside("renew-subscription-login.sh", "renew-subscription-login")
LOGIN_HELPER_PREFIX = "brama-renew-login-"
LOGIN_TAG_PREFIX = "brama:login:"
SUBSCRIPTION_ID_TAG_PREFIX = "brama:id:"
# The tag write belongs to the script that owns tag preservation, which lives in
# the Skarbiec checkout beside this one. It is installed on the host with the
# proven pairs pinned into it, exactly as the login helper is.
MAPPING_HELPER = "map-subscription-logins"
MAPPING_HELPER_SOURCE = (
    pathlib.Path(os.environ["BRAMA_SKARBIEC_SCRIPTS_DIR"]) / "map-subscription-logins.py"
    if os.environ.get("BRAMA_SKARBIEC_SCRIPTS_DIR")
    else beside(
        "map-subscription-logins.py",
        MAPPING_HELPER,
        str(HERE.parent.parent / "skarbiec/scripts/map-subscription-logins.py"),
    )
)
PROVEN_TOKEN = "@PROVEN@"

# Brama's provider ids and the names Weles's reauthentication surface knows them
# by.
WELES_PROVIDERS = {"claude-code": "claude", "codex": "codex", "kimi": "kimi"}

# Which accounts each provider has on this fleet. Weles's `POST /reauth` takes
# the account by its vault login item id, so a sign-in is for a named account and
# the answer echoes which one it drove; this table is what a `brama:login:` tag is
# checked against, so a tag naming an account this fleet does not have is refused
# instead of sent. It is also how the learning pass knows a provider has exactly
# one account left to attribute.
LOGINS_BY_PROVIDER = {
    "claude-code": ("claude-wisent-google-sso", "claude_controlyourai"),
    "codex": ("codex-wisent-google-sso",),
    "kimi": ("kimi-lukasz-google-sso",),
}

# Accounts whose subscription is no longer in service, and the bundle whose
# absence from the live vault says so. `claude_controlyourai` minted the
# subscription of the same name and that bundle is in the vault's trash, so no
# live bundle records the account and no tag ever will - Skarbiec's mapping
# script writes none on a trashed bundle. Without this, the account would read as
# still unattributed and every claude sign-in would look ambiguous. The fact is
# checked rather than trusted: if that bundle is live again, the account is a
# candidate again and the ambiguity is reported instead.
RETIRED_ACCOUNTS = {
    "claude_controlyourai": "provider:claude-code:brama-sub-wisent-app-claude-controlyourai",
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


def host_vault_state(stado: str, host: str, run_id: str) -> tuple[dict, dict]:
    """This host's subscription bundles, live ones and trashed ones apart.

    Both carry revision and tags. The trashed ones settle no renewal - nothing
    serves from them - but they say which accounts still have a subscription in
    service, which is what keeps a retired account out of the learning pass.
    """
    output = run_helper(stado, host, STATE_HELPER, STATE_HELPER_SOURCE, run_id)
    try:
        document = json.loads(output.strip().splitlines()[LAST])
    except (IndexError, ValueError):
        raise Prerequisite(
            f"{host} did not report its subscription bundles as JSON: {output.strip()[:DETAIL]}"
        ) from None
    items = document.get("items")
    trashed = document.get("trashed")
    if not isinstance(items, dict) or not items:
        raise Prerequisite(
            f"the vault at {document.get('vault')} on {host} holds no subscription "
            "bundles, so it is not the vault this gateway serves from"
        )
    return items, trashed if isinstance(trashed, dict) else {}


def bundle_for(items: dict, subscription_id: str) -> tuple[str, dict] | None:
    """The vault bundle a subscription row was served from.

    Matched only on the `brama:id:` tag, because that tag is the contract and
    the only thing that declares which subscription a bundle serves. An item id
    is a mutable human-chosen name: the id-shaped fallback that used to stand
    behind this tag meant a rename silently moved a renewal onto another
    bundle, or a bundle that had quietly lost its enumeration tags kept being
    renewed while the gateway could no longer see it. `brama:id:<id>` is a
    registered tag namespace, so a bundle that ought to be found here can be
    made to carry it; the caller reports a bundle without it as unserved rather
    than guessing.
    """
    wanted = f"{SUBSCRIPTION_ID_TAG_PREFIX}{subscription_id}"
    for name, entry in items.items():
        if wanted in (entry.get("tags") or []):
            return name, entry
    return None


def login_of(entry: dict) -> str | None:
    for tag in entry.get("tags") or []:
        if tag.startswith(LOGIN_TAG_PREFIX):
            return tag[len(LOGIN_TAG_PREFIX):]
    return None


def revision_of(entry: dict | None) -> int:
    """The revision a bundle stands at; a bundle that is not there stands at none."""
    return int((entry or {}).get("revision") or NONE)


def unattributed_logins(provider: str, live: dict, trashed: dict) -> list[str]:
    """This provider's accounts that no bundle on this vault is attributed to.

    An account is attributed when some bundle records it in a `brama:login:` tag,
    live or trashed, and accounted for when the bundle it was established against
    is no longer live. Anything left is an account a sign-in could still turn out
    to belong to, and learning is only sound when exactly one is left.
    """
    recorded = {
        login_of(entry)
        for entry in (*live.values(), *trashed.values())
        if login_of(entry)
    }
    return [
        login
        for login in LOGINS_BY_PROVIDER.get(provider, ())
        if login not in recorded and RETIRED_ACCOUNTS.get(login, "") not in trashed
    ]


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
        self.account = account
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


def cooling_down(persisted: dict, login_item: str) -> int:
    """Seconds left before this account may be signed in again, or none."""
    history = persisted.get(login_item) or {}
    since = time.time() - (history.get("last_attempt_ms") or NONE) / MS_PER_SECOND
    return max(NONE, int(COOLDOWN_SECONDS - since))


def attempt_sign_in(
    stado: str,
    host: str,
    provider: str,
    login_item: str,
    persisted: dict,
    run_id: str,
) -> tuple[int, dict | None]:
    """Record that this account was signed in, then sign it in.

    The attempt is written down before it is made, so a run that dies inside a
    sign-in still leaves the cooldown that keeps the next run from repeating it.
    """
    started_ms = now_ms()
    persisted[login_item] = {
        "last_attempt_ms": started_ms,
        "provider": provider,
        "run": run_id,
    }
    write_state_record(persisted)
    try:
        return started_ms, sign_in(stado, host, provider, login_item, run_id)
    except Prerequisite as error:
        print(f"    the sign-in could not run: {error}")
        return started_ms, None


def record_logins(stado: str, host: str, pairs: list[tuple[str, str]], run_id: str) -> dict:
    """Write the `brama:login:` tag for pairs a sign-in proved.

    The write happens where the vault is, through Skarbiec's own mapping script,
    which preserves every existing tag and goes through `retag` so no payload is
    rewritten. The pairs are pinned into the file that is installed, because
    `run-helper` carries no arguments.
    """
    if not MAPPING_HELPER_SOURCE.is_file():
        raise Prerequisite(
            f"no mapping script at {MAPPING_HELPER_SOURCE}; set "
            "BRAMA_SKARBIEC_SCRIPTS_DIR to the skarbiec checkout that carries it, "
            "because the tag write belongs to the script that owns tag preservation"
        )
    body = MAPPING_HELPER_SOURCE.read_text()
    if PROVEN_TOKEN not in body:
        raise Prerequisite(
            f"{MAPPING_HELPER_SOURCE} carries no {PROVEN_TOKEN} to pin, so a proven "
            "mapping cannot be handed to it"
        )
    proven = ",".join(f"{bundle}={login}" for bundle, login in pairs)
    STATE_DIR.mkdir(parents=True, exist_ok=True)
    rendered = STATE_DIR / MAPPING_HELPER_SOURCE.name
    rendered.write_text(body.replace(PROVEN_TOKEN, proven))
    rendered.chmod(OWNER_ONLY_PROGRAM)
    output = run_helper(stado, host, MAPPING_HELPER, rendered, run_id)
    try:
        return json.loads(output.strip().splitlines()[LAST])
    except (IndexError, ValueError):
        raise Prerequisite(
            f"{host} did not report the tag write as JSON: {output.strip()[:DETAIL]}"
        ) from None


def learn_logins(
    stado: str,
    host: str,
    origin: str,
    agent: str,
    bearer: str,
    secret: str,
    provider: str,
    candidates: list[tuple[str, str, dict, bool]],
    persisted: dict,
    run_id: str,
) -> int:
    """Settle which unmapped subscriptions an account mints, by watching a sign-in.

    Nothing on the vault says which of several subscriptions one login was minted
    from, and no amount of reading will say: the account is only visible in what a
    sign-in writes. So snapshot every candidate bundle, sign in the one account of
    this provider no bundle is attributed to yet, and attribute it to whichever
    candidates advanced a revision - all of them, because one account may
    legitimately mint several. If none advanced, the sign-in produced no
    credential and nothing is written. If two accounts could be responsible, both
    are named and nothing is written: a wrong tag would send every later renewal
    to an account the subscription does not belong to.

    Returns how many refused subscriptions this pass leaves unclosed. A candidate
    that was not refused is never counted: learning which account it belongs to is
    not a repair, and nothing about it was broken.
    """
    print(f"--- learning which {provider} subscriptions an unattributed account mints")
    # The baseline is read here, not at the start of the run: the attempt pass may
    # already have advanced a bundle with a sign-in of its own, and comparing
    # against a stale revision would credit this account with that write.
    try:
        baseline, discarded = host_vault_state(stado, host, run_id)
    except Prerequisite as error:
        print(f"    the host stopped answering before the sign-in: {error}")
        return sum(FIRST for *_, repairable in candidates if repairable)
    watched = [
        (identifier, bundle, baseline[bundle], repairable)
        for identifier, bundle, _, repairable in candidates
        if bundle in baseline and not login_of(baseline[bundle])
    ]
    if not watched:
        print("    every unmapped candidate is either gone from the vault or now mapped")
        return NONE
    refused = sum(FIRST for *_, repairable in watched if repairable)
    if not refused:
        # Nothing here is broken, so nothing here is worth a real sign-in. The
        # mapping is learned on the run that finds one of these refused, which is
        # exactly when it is needed.
        print(
            f"    no unmapped {provider} subscription is refused, so there is nothing "
            "to repair and no sign-in is spent on learning one"
        )
        return NONE
    accounts = LOGINS_BY_PROVIDER.get(provider, ())
    if not accounts:
        # A provider whose accounts this loop does not record cannot be signed in
        # at all, let alone attributed. Named rather than treated as ambiguous, so
        # the answer is "add it here", not "resolve a conflict".
        print(
            f"    this fleet records no {provider} account, so there is no sign-in to "
            "watch; add the provider's accounts to this loop before a subscription of "
            "it can be renewed"
        )
        return refused
    unattributed = unattributed_logins(provider, baseline, discarded)
    if not unattributed:
        # Every account is already attributed to a bundle, so no sign-in can say
        # which account these were minted from: whatever minted them is either an
        # account this fleet does not record or one already recorded elsewhere.
        # A refusal here is not repairable by this loop and is reported as such.
        print(
            f"    unlearnable: every {provider} account this fleet records is already "
            "attributed to a bundle, so no sign-in would say which one minted "
            + ", ".join(bundle for _, bundle, _, _ in watched)
        )
        return refused
    if len(unattributed) != FIRST:
        print(
            f"    ambiguous: {len(unattributed)} {provider} accounts are attributed to "
            f"no bundle on this vault ({', '.join(unattributed)}), so a sign-in would "
            "not say which of them minted a bundle; nothing is written"
        )
        return refused
    login_item = unattributed[NONE]
    remaining = cooling_down(persisted, login_item)
    if remaining:
        # A deferral, not a failure, for the same reason as in the attempt pass.
        print(f"    cooling down: {login_item} is not signed in again for {remaining}s")
        return NONE

    with contextlib.ExitStack() as stack:
        # Every account of this provider is locked, not only the one being signed
        # in: an observation is sound only while no sibling account's login can
        # advance one of these bundles inside the window being watched.
        locks = [
            stack.enter_context(AccountLock(name))
            for name in sorted(accounts)
        ]
        busy = [lock.account for lock in locks if not lock.held]
        if busy:
            print(
                f"    another run is signing in {', '.join(busy)}; a revision that "
                f"moves now could be that run's write, so {login_item} is left to it"
            )
            return NONE
        current = baseline
        started_ms = now_ms()
        advanced: list[tuple[str, str, dict]] = []
        for attempt in range(MAX_ATTEMPTS_PER_ACCOUNT):
            started_ms, answer = attempt_sign_in(
                stado, host, provider, login_item, persisted, run_id
            )
            if answer is None:
                return refused
            print(
                f"    attempt {attempt + FIRST}: signed in {login_item}: "
                f"ok={bool(answer.get('ok'))} confirmed={bool(answer.get('confirmed'))} "
                f"account={answer.get('account') or ACCOUNT_UNCONFIRMED} "
                f"run={answer.get('run_id')} "
                f"detail={str(answer.get('detail') or '')[:DETAIL]}"
            )
            if answer.get("ok") and not answer.get("confirmed"):
                # An unconfirmed run may have signed into any account of this
                # provider, so whatever it wrote proves nothing about who minted
                # it. A release that ignores the selector will ignore it on the
                # next attempt too, so this stops instead of spending another.
                print(
                    "    the run was not confirmed as this account, so nothing is "
                    "attributed; deploy the Weles release that echoes login_item"
                )
                return refused
            if not answer.get("ok"):
                continue
            try:
                current, _ = host_vault_state(stado, host, run_id)
            except Prerequisite as error:
                print(f"    the host stopped answering after the sign-in: {error}")
                return refused
            advanced = [
                (identifier, bundle, entry)
                for identifier, bundle, entry, _ in watched
                if revision_of(current.get(bundle)) > revision_of(entry)
            ]
            # A bundle that already records this account and advanced too is worth
            # saying out loud: it is the same credential write, and it confirms the
            # account rather than adding a mapping.
            for bundle, entry in sorted(baseline.items()):
                if login_of(entry) != login_item:
                    continue
                if revision_of(current.get(bundle)) > revision_of(entry):
                    print(f"    {bundle} advanced too, and already records this account")
            if advanced:
                break
        if not advanced:
            print(
                f"    the sign-in for {login_item} produced no credential: no candidate "
                "bundle advanced a revision, so nothing is attributed and no tag is written"
            )
            return refused

        # Between the snapshot and the write, another run may have attributed one
        # of these bundles. A bundle that now names a different account is left
        # alone: two accounts cannot both have minted it, and nothing observed here
        # says the record already on it is the wrong one.
        conflicted = {
            bundle: login_of(current[bundle])
            for _, bundle, _ in advanced
            if login_of(current.get(bundle) or {}) not in (None, login_item)
        }
        for bundle, other in sorted(conflicted.items()):
            print(
                f"    conflict: {bundle} advanced, but it now records "
                f"{LOGIN_TAG_PREFIX}{other}, which is not the account this sign-in "
                "drove; leaving its tag alone"
            )
        provable = [
            (identifier, bundle, entry)
            for identifier, bundle, entry in advanced
            if bundle not in conflicted
        ]
        if not provable:
            print("    every bundle this sign-in advanced is already attributed elsewhere")
            return refused
        try:
            written = record_logins(
                stado, host, [(bundle, login_item) for _, bundle, _ in provable], run_id
            )
        except Prerequisite as error:
            print(f"    the tag write could not run: {error}")
            return refused
        for conflict in written.get("conflicts") or []:
            print(f"    {conflict}")
        wrote = set(written.get("written") or [])
        already = set(written.get("already") or [])
        for _, bundle, _ in provable:
            if bundle in wrote:
                print(f"    wrote {LOGIN_TAG_PREFIX}{login_item} on {bundle}")
            elif bundle in already:
                print(f"    {bundle} already recorded {LOGIN_TAG_PREFIX}{login_item}")
        missing = [bundle for _, bundle, _ in provable if bundle not in wrote | already]
        for bundle in missing:
            print(
                f"    {bundle} was proven to belong to {login_item}, but the tag write "
                "did not report it, so it stays unmapped and unrenewable"
            )

        # Only the candidates that were actually refused are put through the
        # renewal verification: a subscription nobody refused has nothing to close,
        # and waiting for its next probe would report a failure that is not one.
        broken = {identifier for identifier, _, _, repairable in watched if repairable}
        renewed = [
            (identifier, bundle, entry)
            for identifier, bundle, entry in provable
            if identifier in broken
        ]
        if not renewed:
            return refused + len(missing)
        closed = verify(
            stado,
            host,
            origin,
            agent,
            bearer,
            secret,
            {"provider": provider, "subscriptions": renewed},
            started_ms,
            run_id,
        )
        return refused - closed + len(missing)


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
        vault_before, _ = host_vault_state(stado, host, run_id)
    except Prerequisite as error:
        print(str(error), file=sys.stderr)
        return len(["prerequisite"])

    print(f"agent:  {agent} at {origin}")
    print(f"host:   {host}")
    print(f"run:    {run_id}")

    persisted = read_state_record()
    attempted: dict[str, dict] = {}
    unmapped: dict[str, list[tuple[str, str, dict, bool]]] = {}
    unclosed = NONE

    for subscription in sorted(listing["subscriptions"], key=lambda entry: str(entry.get("id"))):
        identifier = str(subscription.get("id"))
        provider = str(subscription.get("provider"))
        state = state_of(subscription)
        detail = probe_detail(subscription)
        repairable = state == STATE_REFUSED and is_auth_refusal(detail)
        print(f"=== {identifier} ({provider})")
        found = bundle_for(vault_before, identifier)
        if state == STATE_REFUSED and not repairable:
            print(f"    refused for a reason a sign-in does not repair: {detail[:DETAIL]}")
            continue
        if not found:
            if repairable:
                print(
                    "    refused, but no vault bundle on this host declares "
                    f"{SUBSCRIPTION_ID_TAG_PREFIX}{identifier}, so there is nothing to "
                    "renew and nothing to verify; if a bundle does serve it, tag it"
                )
                unclosed += FIRST
            else:
                print(f"    {state}: nothing to renew")
            continue
        bundle, entry = found
        login_item = login_of(entry)
        if not login_item:
            # A candidate for the learning pass: which account mints it is exactly
            # what a sign-in can settle, and until it is settled no login is
            # attempted on its behalf. A blocked subscription is not a candidate:
            # the loop acts on no account while a block is in force, and a block
            # lifts long before an account stops existing.
            if state == STATE_BLOCKED:
                print(
                    f"    unmapped and blocked: {bundle} carries no {LOGIN_TAG_PREFIX} "
                    "tag, and nothing is signed in for it while the block is in force"
                )
                continue
            unmapped.setdefault(provider, []).append((identifier, bundle, entry, repairable))
            print(
                f"    unmapped: {bundle} carries no {LOGIN_TAG_PREFIX} tag, so the "
                f"account it signs in with is not recorded yet ({state}); one sign-in "
                "of the account nothing is attributed to yet would settle it"
            )
            continue
        if not repairable:
            print(f"    {state}: nothing to renew; account {login_item}")
            continue
        if login_item not in LOGINS_BY_PROVIDER.get(provider, ()):
            print(
                f"    unattributable: {bundle} names account {login_item}, which is "
                f"not one this fleet records for {provider}; refusing to sign in"
            )
            unclosed += FIRST
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
        remaining = cooling_down(persisted, login_item)
        if remaining:
            # A deferral, not a failure: the previous run already reported what it
            # found, and a timer that alerted on every run inside the cooldown
            # would alert for an hour about one refusal.
            print(f"    cooling down: this account is not signed in again for {remaining}s")
            continue
        with AccountLock(login_item) as lock:
            if not lock.held:
                print("    another run is already signing this account in; leaving it to that run")
                continue
            closed = NONE
            for attempt in range(MAX_ATTEMPTS_PER_ACCOUNT):
                started_ms, answer = attempt_sign_in(
                    stado, host, provider, login_item, persisted, run_id
                )
                if answer is None:
                    break
                print(
                    f"    attempt {attempt + FIRST}: ok={bool(answer.get('ok'))} "
                    f"confirmed={bool(answer.get('confirmed'))} "
                    f"account={answer.get('account') or ACCOUNT_UNCONFIRMED} "
                    f"run={answer.get('run_id')} "
                    f"detail={str(answer.get('detail') or '')[:DETAIL]}"
                )
                closed = verify(
                    stado, host, origin, agent, bearer, secret, plan, started_ms, run_id
                )
                if closed == len(plan["subscriptions"]):
                    break
            unclosed += len(plan["subscriptions"]) - closed

    # Learning comes last, and takes its own snapshot: the attempt pass may have
    # advanced a bundle already, and a candidate that is now mapped is no longer a
    # candidate. The two passes never sign the same account in twice in one run -
    # the attempt pass only signs in accounts a bundle already records, and
    # learning only signs in an account no bundle records at all.
    for provider in sorted(unmapped):
        unclosed += learn_logins(
            stado,
            host,
            origin,
            agent,
            bearer,
            secret,
            provider,
            unmapped[provider],
            persisted,
            run_id,
        )

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
        vault_after, _ = host_vault_state(stado, host, run_id)
    except Prerequisite as error:
        print(f"    the host stopped answering while verifying: {error}")
        return NONE

    closed = NONE
    for identifier, bundle, before in plan["subscriptions"]:
        was = revision_of(before)
        now = revision_of(vault_after.get(bundle))
        subscription = fresh.get(identifier)
        if subscription is None:
            print(
                f"    {identifier}: the gateway has not checked it since the login "
                f"(waited {PROBE_WAIT_SECONDS}s); revision {was} -> {now}. The gateway reads "
                "each provider's own usage report on a timer; set "
                "BRAMA_PLAN_USAGE_SWEEP_SECS if that sweep is off, or trigger the paid check "
                "with POST /v1/admin/subscriptions/<agent>/<subscription>/probe"
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
