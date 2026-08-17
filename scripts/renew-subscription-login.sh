#!/bin/sh
# Ask Weles, on the host Weles runs on, to sign one named account in, and report
# which account it actually signed into.
#
# This is the login half of the renewal loop. Brama cannot repair a refused
# subscription credential by itself: claude and kimi have no local CLI to
# refresh, and a revoked OAuth token is only replaced by a real sign-in. Weles
# owns that sign-in, drives it in its own browser on its own host, and exposes it
# as `POST /reauth` on its worker API. Nothing here opens a browser, and no
# credential value is read or printed.
#
# The account is named by its vault login item id, in `login_item`, which Weles
# resolves to that account's own sign-in row. Weles echoes `login_item` and
# `display_name` back, and this reports the run as confirmed only when the echo
# is the account that was asked for. A release that predates the selector ignores
# the field and answers no `login_item` at all: that run is reported unconfirmed,
# which is what stops the loop from attributing a credential to an account nobody
# proved it came from.
#
# `stado host run-helper` passes no arguments beyond correlation UUIDs and no
# caller environment, so the provider and the account this run is for are pinned
# into this file when it is installed - the same way the managed-release helper
# pins its revision. Running it unrendered is refused rather than defaulted: a
# helper that guesses a provider is a helper that signs into the wrong account.
# Both values can also come from the environment, which is what makes this file
# runnable by hand on the host while staying reviewable in the repository.
#
# Emits one JSON object on standard output:
#   {"ok":bool,"confirmed":bool,"provider":..,"login_item":..,
#    "signed_in_login_item":..,"account":..,"http_status":N,"run_id":..,
#    "refreshed":bool,"timed_out":bool,"detail":".."}
set -eu

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

PROVIDER="${BRAMA_RENEWAL_PROVIDER:-@PROVIDER@}"
LOGIN_ITEM="${BRAMA_RENEWAL_LOGIN_ITEM:-@LOGIN_ITEM@}"
# Weles's own default worker API address and its own default run budget. A
# sign-in drives a real browser through Google SSO and a consent screen, so the
# budget is minutes, not seconds.
WELES_HOST="${WELES_API_HOST:-127.0.0.1}"
WELES_PORT="${WELES_API_PORT:-8788}"
# Where the worker's own API token lives. Weles's launcher sources several files
# and the later one wins; on this fleet the token is minted into
# ~/.weles/secrets.env, while ~/.config/weles/worker.env carries release pins and
# no token at all. Naming one file made this helper report "no worker API token"
# on a host that had one, so it reads the same set in the same order.
WORKER_ENV_FILES="${WELES_WORKER_ENV_FILE:-}"
[ -n "$WORKER_ENV_FILES" ] || WORKER_ENV_FILES="$HOME/weles/var/worker-content.env
$HOME/.config/weles/worker.env
$HOME/.weles/secrets.env
$HOME/.stado/weles-model.env"
LOGIN_TIMEOUT_MS="${BRAMA_RENEWAL_LOGIN_TIMEOUT_MS:-900000}"
TRANSPORT_TIMEOUT_SECONDS="${BRAMA_RENEWAL_TRANSPORT_TIMEOUT_SECONDS:-1200}"

case "$PROVIDER" in
  @*)
    printf 'this helper was installed unrendered: no provider is pinned in it\n' >&2
    exit 1
    ;;
  claude | codex | kimi) ;;
  *)
    printf 'Weles reauthenticates claude, codex and kimi; %s is not one of them\n' "$PROVIDER" >&2
    exit 1
    ;;
esac
case "$LOGIN_ITEM" in
  @* | "")
    printf 'this helper was installed unrendered: no login item is pinned in it\n' >&2
    exit 1
    ;;
esac

present=
token=
for candidate in $WORKER_ENV_FILES; do
  [ -f "$candidate" ] || continue
  present="$candidate"
  # The token guards Weles's worker API. It is read into a variable and then into
  # an owner-only curl configuration file, so it reaches neither this script's
  # arguments nor curl's, and never the process table or a log line.
  found=$(sed -n 's/^ *export *WELES_API_TOKEN=//p;s/^WELES_API_TOKEN=//p' "$candidate" | tail -1 | tr -d '"')
  [ -n "$found" ] || found=$(sed -n 's/^ *export *WELES_CONSOLE_API_TOKEN=//p;s/^WELES_CONSOLE_API_TOKEN=//p' "$candidate" | tail -1 | tr -d '"')
  [ -n "$found" ] && token="$found"
done
[ -n "$present" ] || {
  printf 'no Weles worker environment file exists on this host, so it has no worker API token\n' >&2
  exit 1
}
[ -n "${token:-}" ] || {
  printf 'no WELES_API_TOKEN or WELES_CONSOLE_API_TOKEN in any of: %s\n' "$(printf '%s' "$WORKER_ENV_FILES" | tr '\n' ' ')" >&2
  exit 1
}

BASE="http://$WELES_HOST:$WELES_PORT"
umask 077
work=$(mktemp -d "${TMPDIR:-/tmp}/brama-renew-login.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

# `-f` matters: without it curl reports success for any answer at all, and a
# healthy exit code from an unrelated service that happens to hold this port
# would send a sign-in request nobody serves.
health=$(/usr/bin/curl -f -s -m "$TRANSPORT_TIMEOUT_SECONDS" -o "$work/health.json" \
  -w '%{http_code}' "$BASE/healthz" || true)
[ "$health" = "200" ] || {
  printf 'Weles worker API does not answer its own health check at %s (%s); start it before renewing a login\n' \
    "$BASE/healthz" "${health:-no answer}" >&2
  exit 1
}

# Everything that can be known before a browser opens is checked here, because
# the cost of finding out afterwards is one real sign-in into the wrong account.
# The health answer advertises whether this release honours the `login_item`
# selector and which sign-in rows it holds; a release without the selector would
# silently pick a row of its own, and a named account with no row cannot be signed
# in at all.
LOGIN_ITEM="$LOGIN_ITEM" PROVIDER="$PROVIDER" /usr/bin/python3 - "$work/health.json" <<'PY' || exit 1
import json
import os
import sys

SELECTOR = "login_item"
try:
    health = json.loads(open(sys.argv[len(["self"])], "r", encoding="utf-8").read())
except (OSError, ValueError):
    health = {}
asked = os.environ[SELECTOR.upper()]
provider = os.environ["PROVIDER"]
features = health.get("features") if isinstance(health, dict) else None
if not isinstance(features, list) or SELECTOR not in features:
    sys.stderr.write(
        f"this Weles release does not advertise the {SELECTOR} selector, so it would "
        f"choose a sign-in row itself instead of using {asked}; deploy the release "
        "that carries it before renewing a named account\n"
    )
    raise SystemExit(len(["refused"]))
rows = health.get("login_items")
rows = rows if isinstance(rows, list) else []
named = [row for row in rows if isinstance(row, dict) and row.get(SELECTOR) == asked]
if not named:
    sys.stderr.write(
        f"Weles holds no sign-in row for {asked}; it holds "
        + (", ".join(str(row.get(SELECTOR)) for row in rows) or "none")
        + ". That account has to exist in Weles before it can be signed in\n"
    )
    raise SystemExit(len(["refused"]))
mismatched = [row for row in named if row.get("provider") not in (None, provider)]
if mismatched and len(mismatched) == len(named):
    sys.stderr.write(
        f"{asked} is a {mismatched[len([])].get('provider')} account, not a {provider} "
        "one; refusing to sign it in for the wrong provider\n"
    )
    raise SystemExit(len(["refused"]))
PY

printf 'header = "Authorization: Bearer %s"\n' "$token" > "$work/curl.conf"
printf '{"provider":"%s","login_item":"%s","timeout_ms":%s}\n' \
  "$PROVIDER" "$LOGIN_ITEM" "$LOGIN_TIMEOUT_MS" > "$work/body.json"

status=$(/usr/bin/curl -s -m "$TRANSPORT_TIMEOUT_SECONDS" -X POST "$BASE/reauth" \
  --config "$work/curl.conf" \
  -H 'Content-Type: application/json' \
  --data @"$work/body.json" \
  -o "$work/answer.json" \
  -w '%{http_code}' || true)

PROVIDER="$PROVIDER" LOGIN_ITEM="$LOGIN_ITEM" HTTP_STATUS="$status" \
  /usr/bin/python3 - "$work/answer.json" <<'PY'
import json
import os
import re
import sys

DETAIL = int("400")
OK_STATUS = "200"
# The account Weles says it drove. `display_name` is its own row name and
# `login_item` is the vault id this asked for; a release that predates the
# selector answers neither, and the trajectories' own log line is then the only
# statement of what ran - reported, but never treated as confirmation.
ACCOUNT_LINE = re.compile(r"reauthing LRU row ([^(\n]+)")

status = os.environ["HTTP_STATUS"]
try:
    answer = json.loads(open(sys.argv[len(["self"])], "r", encoding="utf-8").read())
except (OSError, ValueError):
    answer = {}
if not isinstance(answer, dict):
    answer = {}

tail = str(answer.get("stdout_tail") or "")
match = ACCOUNT_LINE.search(tail)
asked = os.environ["LOGIN_ITEM"]
signed_in = answer.get("login_item")
account = answer.get("display_name") or (
    match.group(len(["whole"])).strip() if match else None
)


def last_line(text):
    lines = [line.strip() for line in str(text or "").splitlines() if line.strip()]
    return lines[-len(["last"])] if lines else ""


# What went wrong, in one sentence: the API's own error when it named one, else
# the last thing the run said, because a whole log tail is not a report.
detail = str(answer.get("error") or "") or last_line(answer.get("stderr_tail")) or last_line(tail)
ran = status == OK_STATUS and bool(answer.get("ok"))
confirmed = ran and signed_in == asked
if ran and not confirmed:
    detail = (
        f"this Weles release signed in as {signed_in or account or 'an unnamed account'} "
        f"without confirming {asked}, so the run cannot be attributed to it"
    )
json.dump(
    {
        "ok": ran,
        "confirmed": confirmed,
        "provider": os.environ["PROVIDER"],
        "login_item": asked,
        "signed_in_login_item": signed_in,
        "account": account,
        "http_status": status,
        "run_id": answer.get("run_id"),
        "refreshed": bool(answer.get("refreshed")),
        "timed_out": bool(answer.get("timed_out")),
        "detail": detail[:DETAIL],
    },
    sys.stdout,
)
sys.stdout.write("\n")
PY
