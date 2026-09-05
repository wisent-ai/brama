#!/usr/bin/env bash
# Report which vendor sessions this host can refresh without a browser.
#
# The Codex credential was repaired by asking its own CLI to exchange the
# refresh token -- no login, no browser. Whether the same shortcut exists for
# the other subscriptions decides whether their repair is a local command or a
# Weles trajectory driving a real sign-in, and that is not something to assume.
#
# Read-only. Prints tool paths and session file times; no credential content.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

report() {
  name=$1
  binary=$2
  session=$3
  echo "=== $name ==="
  path=$(command -v "$binary" 2>/dev/null || true)
  if [ -n "$path" ]; then echo "cli: $path"; else echo "cli: absent"; fi
  if [ -e "$session" ]; then
    /bin/ls -ld "$session" | /usr/bin/awk '{print "session: "$6" "$7" "$8"  "$5" bytes"}'
  else
    echo "session: absent ($session)"
  fi
  echo
}

report codex codex "$HOME/.codex/auth.json"
report claude claude "$HOME/.claude/.credentials.json"
report claude-alt claude "$HOME/.config/claude/credentials.json"
report kimi kimi "$HOME/.kimi/credentials.json"

echo "=== other vendor state directories present ==="
for dir in "$HOME/.claude" "$HOME/.kimi" "$HOME/.moonshot" "$HOME/.anthropic"; do
  [ -d "$dir" ] && /bin/ls -1 "$dir" | head -6 | /usr/bin/sed "s|^|$(basename "$dir")/|"
done
echo "(nothing above means no local session state for those vendors)"
