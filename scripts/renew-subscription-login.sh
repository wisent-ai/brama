#!/bin/sh
# Ask Weles, on the host Weles runs on, to sign in again for one provider and
# report which account it used.
#
# This is the login half of the renewal loop. Brama cannot repair a refused
# subscription credential by itself: claude and kimi have no local CLI to
# refresh, and a revoked OAuth token is only replaced by a real sign-in. Weles
# owns that sign-in, drives it in its own browser on its own host, and exposes
# it as `POST /reauth` on its worker API. Nothing here opens a browser, and no
# credential value is read or printed.
#
# `stado host run-helper` passes no arguments beyond correlation UUIDs and no
# caller environment, so the provider and the account this run is for are pinned
# into this file when it is installed - the same way the managed-release helper
# pins its revision. Running it unrendered is refused rather than defaulted:
# a helper that guesses a provider is a helper that signs into the wrong
# account. Both values can also come from the environment, which is what makes
# this file runnable by hand on the host while staying reviewable in the
# repository.
#
# Emits one JSON object on standard output:
#   {"ok":bool,"provider":..,"login_item":..,"http_status":N,"run_id":..,
#    "account":..,"refreshed":bool,"detail":".."}
# `account` is the account Weles reports it signed into, so a run that landed on
# another account is visible in the loop's report instead of passing as success.
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
WORKER_ENV="${WELES_WORKER_ENV_FILE:-$HOME/.config/weles/worker.env}"
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

[ -f "$WORKER_ENV" ] || {
  printf 'no Weles worker environment at %s, so this host has no worker API token\n' "$WORKER_ENV" >&2
  exit 1
}
# The token guards Weles's worker API. It is read into a variable and then into
# an owner-only curl configuration file, so it reaches neither this script's
# arguments nor curl's, and never the process table or a log line.
token=$(sed -n 's/^WELES_API_TOKEN=//p' "$WORKER_ENV" | tail -1 | tr -d '"')
[ -n "${token:-}" ] || token=$(sed -n 's/^WELES_CONSOLE_API_TOKEN=//p' "$WORKER_ENV" | tail -1 | tr -d '"')
[ -n "${token:-}" ] || {
  printf 'neither WELES_API_TOKEN nor WELES_CONSOLE_API_TOKEN is set in %s\n' "$WORKER_ENV" >&2
  exit 1
}

BASE="http://$WELES_HOST:$WELES_PORT"
# `-f` matters: without it curl reports success for any answer at all, and a
# healthy exit code from an unrelated service that happens to hold this port
# would send a sign-in request nobody serves.
health=$(/usr/bin/curl -f -s -m "$TRANSPORT_TIMEOUT_SECONDS" -o /dev/null -w '%{http_code}' \
  "$BASE/healthz" || true)
[ "$health" = "200" ] || {
  printf 'Weles worker API does not answer its own health check at %s (%s); start it before renewing a login\n' \
    "$BASE/healthz" "${health:-no answer}" >&2
  exit 1
}

umask 077
work=$(mktemp -d "${TMPDIR:-/tmp}/brama-renew-login.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM
printf 'header = "Authorization: Bearer %s"\n' "$token" > "$work/curl.conf"
printf '{"provider":"%s","timeout_ms":%s}\n' "$PROVIDER" "$LOGIN_TIMEOUT_MS" > "$work/body.json"

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
# What the reauth trajectories print when they pick the account they will drive:
# `[reauth] expiring-soon - reauthing LRU row <display name> (updated ...)`. It
# is the only place the account that was actually signed into is stated, and the
# loop needs it to see a run that landed on another account.
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


def last_line(text):
    lines = [line.strip() for line in str(text or "").splitlines() if line.strip()]
    return lines[-len(["last"])] if lines else ""


# What went wrong, in one sentence: the API's own error when it named one, else
# the last thing the run said, because a whole log tail is not a report.
detail = str(answer.get("error") or "") or last_line(answer.get("stderr_tail")) or last_line(tail)
json.dump(
    {
        "ok": status == OK_STATUS and bool(answer.get("ok")),
        "provider": os.environ["PROVIDER"],
        "login_item": os.environ["LOGIN_ITEM"],
        "http_status": status,
        "run_id": answer.get("run_id"),
        "account": match.group(len(["whole"])).strip() if match else None,
        "refreshed": bool(answer.get("refreshed")),
        "timed_out": bool(answer.get("timed_out")),
        "detail": detail[:DETAIL],
    },
    sys.stdout,
)
sys.stdout.write("\n")
PY
