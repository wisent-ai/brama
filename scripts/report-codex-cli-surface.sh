#!/usr/bin/env bash
# Report the Codex CLI's command surface on this host.
#
# Refreshing a lapsed session unattended means picking the one command that
# exchanges the refresh token and does nothing else. Guessing that command is
# how an agent opens a browser on a machine nobody is sitting at, so the help
# text is read first.
#
# Read-only: prints help output only.
set -u
# The CLI is a node program and helpers run with a minimal PATH, so `env node`
# inside its shebang fails before it prints anything.
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

CODEX=/opt/homebrew/bin/codex
[ -x "$CODEX" ] || { echo "no codex at $CODEX"; exit 0; }

echo "=== version ==="
"$CODEX" --version 2>&1 | head -2

echo
echo "=== commands ==="
"$CODEX" --help 2>&1 | /usr/bin/sed -n '/[Cc]ommands:/,/^$/p' | head -24

echo
echo "=== login subcommands ==="
"$CODEX" login --help 2>&1 | head -20
