#!/usr/bin/env bash
# Print what the Codex credential healer has done.
#
# The healer is woken by launchd on writes to the dispatcher's usage ledger, so
# it leaves no trace in any service log the fleet already reads. Without this
# the only evidence that it ran is the credential quietly working again.
#
# Read-only.
set -u

LABEL="com.wisent.always-on.codex-credential-healer"
LOG="$HOME/.stado/logs/codex-credential-healer.log"

echo "=== unit ==="
/bin/launchctl print "system/${LABEL}" 2>/dev/null | /usr/bin/sed -n '/state = /p;/last exit code/p' | head -2
echo "(no lines above means launchd does not have it loaded)"

echo
echo "=== log tail ==="
if [ -f "$LOG" ]; then
  /usr/bin/tail -n "$(printf '%s' "$(/usr/bin/wc -l < "$LOG")" | /usr/bin/awk '{print ($1 > 20) ? 20 : $1}')" "$LOG"
else
  echo "(no log at $LOG yet)"
fi
