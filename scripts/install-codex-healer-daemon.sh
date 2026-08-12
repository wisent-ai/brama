#!/usr/bin/env bash
# Install the Codex credential healer as a launchd daemon woken by the ledger.
#
# `stado service deploy` renders a LaunchAgent, and bootstrapping one over ssh
# fails with "could not switch to audit session" because there is no GUI
# session to bootstrap into. Every always-on unit in this fleet is therefore a
# system daemon, and this follows them.
#
# The job is not a timer. launchd watches the dispatcher's usage ledger and
# starts it on every write, which is the exact moment a refusal is recorded, so
# the healer runs when a credential is actually refused and never on a clock.
#
# Idempotent: re-running replaces the definition and reloads it.
set -eu

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

LABEL="com.wisent.always-on.codex-credential-healer"
PLIST="/Library/LaunchDaemons/${LABEL}.plist"
HEALER="$HOME/.stado/bin/heal-codex-subscription"
LEDGER="$HOME/.config/brama/subscription-usage.json"
LOG="$HOME/.stado/logs/codex-credential-healer.log"
PYTHON=/usr/bin/python3

[ -x "$HEALER" ] || { echo "healer is not installed at $HEALER" >&2; exit 1; }
[ -x "$PYTHON" ] || { echo "no python at $PYTHON" >&2; exit 1; }

sudo=""
if [ "$(/usr/bin/id -u)" != "0" ]; then sudo="/usr/bin/sudo -n"; fi
owner=$(/usr/bin/id -un)

/bin/mkdir -p "$(/usr/bin/dirname "$LOG")"

tmp=$(/usr/bin/mktemp)
/bin/cat > "$tmp" <<PLISTEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${PYTHON}</string>
        <string>${HEALER}</string>
        <string>--once</string>
    </array>
    <key>UserName</key>
    <string>${owner}</string>
    <key>WatchPaths</key>
    <array>
        <string>${LEDGER}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${LOG}</string>
    <key>StandardErrorPath</key>
    <string>${LOG}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>${HOME}</string>
        <key>PATH</key>
        <string>${PATH}</string>
    </dict>
</dict>
</plist>
PLISTEOF

$sudo /bin/cp "$tmp" "$PLIST"
$sudo /usr/sbin/chown root:wheel "$PLIST"
$sudo /bin/chmod go-w "$PLIST"
/bin/rm -f "$tmp"

$sudo /bin/launchctl bootout "system/${LABEL}" 2>/dev/null || true
$sudo /bin/launchctl bootstrap system "$PLIST"

echo "installed: $PLIST"
echo "watching:  $LEDGER"
echo "log:       $LOG"
/bin/launchctl print "system/${LABEL}" 2>/dev/null | /usr/bin/sed -n '/state = /p' | head -1
