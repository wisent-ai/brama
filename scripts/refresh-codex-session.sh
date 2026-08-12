#!/usr/bin/env bash
# Refresh this host's Codex session through the vendor CLI, without a login.
#
# The gateway sends `tokens.access_token` verbatim, so a subscription serves
# until that short-lived token lapses and then reports "authentication token is
# expired" -- which reads as a dead account and is not one. The session file
# carries a refresh_token and the CLI exchanges it on any authenticated call,
# so the repair is a token exchange, not a browser.
#
# `login status` is the read-only call: it reports the account and exchanges a
# lapsed token on the way. Nothing here runs a model or opens a browser.
#
# Prints the session's refresh timestamp before and after. No token is printed.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

CODEX=/opt/homebrew/bin/codex
AUTH="$HOME/.codex/auth.json"
[ -x "$CODEX" ] || { echo "no codex CLI at $CODEX"; exit 1; }
[ -f "$AUTH" ] || { echo "no session at $AUTH"; exit 1; }

stamp() { /usr/bin/grep -o '"last_refresh"[^,]*' "$AUTH" | head -1; }

echo "before: $(stamp)"
echo "--- codex login status ---"
"$CODEX" login status 2>&1 | head -4

# `login status` proves the account is alive but does not exchange anything;
# the CLI refreshes on an authenticated call. This is the smallest one there
# is: a fixed one-word prompt, no tools, no sandbox writes. It spends a few
# tokens of a subscription that is already paid for, which is the whole point
# of holding it.
echo "--- forcing an exchange ---"
"$CODEX" exec --skip-git-repo-check "Reply with the single word PROBE." 2>&1 | tail -3

echo "--- after ---"
echo "after:  $(stamp)"
