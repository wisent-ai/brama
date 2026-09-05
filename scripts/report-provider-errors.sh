#!/usr/bin/env bash
# Print the provider errors the gateway recorded, grouped, newest kinds last.
#
# A credential that fails without an authentication block never triggers the
# forced OAuth refresh, so it stays broken while looking like a credential
# nobody tried. The dispatcher's answer to the caller collapses every cause into
# "all bounded credentials unavailable", so the provider's own words are the
# only thing that separates a stale token from a rejected request shape.
#
# Read-only. Both gateway streams are read: launchd does not consistently give
# these lines to the error one.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

LOGS="$HOME/.stado/logs/brama-always-on.err $HOME/.stado/logs/brama-always-on.out"

echo "=== refresh failures by provider and reason ==="
# The lines carry terminal escapes between fields, so provider and error are
# pulled out separately rather than with one anchored pattern.
# shellcheck disable=SC2086
/usr/bin/grep -h 'oauth_refresh_failed' $LOGS 2>/dev/null \
  | /usr/bin/sed -n 's/.*provider="\([a-z-]*\)".*error=\(.*\)$/\1: \2/p' \
  | /usr/bin/sort \
  | /usr/bin/uniq -c \
  | /usr/bin/sort -rn \
  | /usr/bin/head -n 8
echo "(empty means no refresh failure carried both fields)"

echo
echo "=== lines naming claude, newest last ==="
# shellcheck disable=SC2086
/usr/bin/grep -h 'claude' $LOGS 2>/dev/null \
  | /usr/bin/grep -E 'credential|refresh|provider|blocked|unavailable' \
  | /usr/bin/tail -n 6 \
  | /usr/bin/cut -c1-400
