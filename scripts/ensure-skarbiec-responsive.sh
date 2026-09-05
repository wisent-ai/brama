#!/bin/bash
set -euo pipefail

PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
export PATH

url=${SKARBIEC_HEALTH_URL:-http://127.0.0.1:8895/health}
if curl --fail --silent --max-time 5 "$url" >/dev/null; then
  printf 'skarbiec already responsive\n'
  exit 0
fi

uid=$(id -u)
label=''
domain=''
for plist in "$HOME"/Library/LaunchAgents/*.plist /Library/LaunchDaemons/*.plist; do
  [ -f "$plist" ] || continue
  candidate=$(/usr/libexec/PlistBuddy -c 'Print :Label' "$plist" 2>/dev/null || true)
  arguments=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' "$plist" 2>/dev/null || true)
  case "$arguments" in
    *skarbiec*serve*8895*)
      label=$candidate
      case "$plist" in
        /Library/LaunchDaemons/*) domain=system ;;
        *) domain="gui/$uid" ;;
      esac
      break
      ;;
  esac
done

[ -n "$label" ] || {
  printf 'no launchd unit owns the Skarbiec listener on port 8895\n' >&2
  exit 69
}

# Preserve the loaded definition. Unload/bootstrap can destroy the only known
# working state; kickstart replaces only the unhealthy process in place.
launchctl kickstart -k "$domain/$label"

for attempt in 1 2 3 4 5 6 7 8 9 10; do
  if curl --fail --silent --max-time 5 "$url" >/dev/null; then
    printf 'skarbiec responsive after in-place recovery (%s)\n' "$label"
    exit 0
  fi
  sleep 2
done

printf 'skarbiec unit %s did not become responsive\n' "$label" >&2
exit 69
