#!/usr/bin/env bash
# Report whether this host can refresh its Codex session without a login.
#
# The gateway sends `tokens.access_token` verbatim and never refreshes it, so a
# subscription works until that short-lived token lapses and then fails as
# "authentication token is expired" -- indistinguishable, from the caller, from
# a revoked account. The session file carries a refresh_token, and the vendor
# CLI exchanges it unattended. Whether that CLI is here decides if the repair
# is a token exchange or a browser login.
#
# Read-only. Prints tool paths, versions and the session's age; no token.
set -u

echo "=== codex cli ==="
for candidate in codex codex-cli; do
  path=$(command -v "$candidate" 2>/dev/null || true)
  [ -n "$path" ] && echo "$candidate: $path"
done
for path in "$HOME/.local/bin/codex" /opt/homebrew/bin/codex /usr/local/bin/codex "$HOME/.npm-global/bin/codex"; do
  [ -x "$path" ] && echo "found: $path"
done
echo "(no lines above means no codex CLI on this host)"

echo
echo "=== npm global packages that look like it ==="
if command -v npm >/dev/null; then
  npm ls -g --depth=0 2>/dev/null | /usr/bin/grep -i -E 'codex|openai' | head -4
else
  echo "(npm unavailable)"
fi

echo
echo "=== session file ==="
auth="$HOME/.codex/auth.json"
if [ -f "$auth" ]; then
  /bin/ls -l "$auth" | /usr/bin/awk '{print "written: "$6" "$7" "$8"  bytes: "$5}'
  /usr/bin/grep -o '"last_refresh"[^,]*' "$auth" | head -1
  for field in refresh_token id_token access_token; do
    /usr/bin/grep -q "\"$field\"" "$auth" && echo "carries: $field"
  done
else
  echo "absent: $auth"
fi
