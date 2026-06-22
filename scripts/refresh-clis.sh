#!/bin/sh
# Atomically refresh the subscription CLIs (claude-code / codex / opencode)
# WITHOUT ever leaving the live binaries at /usr/local/bin/{claude,codex,
# opencode} missing.
#
# Why this exists: the previous approach ran `npm install -g <pkg>@latest`
# directly against the global prefix on every container boot and every 6h.
# npm unlinks-then-relinks the package during that install, so for a few
# seconds /usr/local/bin/claude does not exist. A subscription dispatch that
# spawns `claude` in that window dies with `spawn: No such file or directory`
# (ENOENT). With low Cloud Run concurrency, cold containers still inside their
# boot-time install window served real traffic and failed.
#
# Fix: install into an INACTIVE slot, then atomically flip the symlinks. The
# slot the live symlink currently points at is never mutated, so a concurrent
# `claude -p` always resolves a complete binary. The build-time `npm install
# -g` keeps the binaries present from t=0 until the first staged flip. If the
# npm install fails, the symlinks are left untouched (current version stays
# live) — best-effort, never an outage.
set -e

SLOT_A=/opt/cli-a
SLOT_B=/opt/cli-b
PKGS="@anthropic-ai/claude-code@latest @openai/codex@latest opencode-ai@latest @moonshot-ai/kimi-code@latest"
BINS="claude codex opencode kimi"

# Pick the inactive slot: whichever the live `claude` symlink does NOT
# currently resolve into. On first run the live binary is the build-time
# install under /usr/local/lib, matching neither slot, so we pick SLOT_A.
cur="$(readlink -f /usr/local/bin/claude 2>/dev/null || echo '')"
case "$cur" in
  "$SLOT_A"/*) next="$SLOT_B" ;;
  *)           next="$SLOT_A" ;;
esac

rm -rf "$next"
mkdir -p "$next"

# Stage the new versions into the inactive slot. Failure leaves the live
# symlinks untouched.
if npm install -g --prefix "$next" $PKGS --no-fund --no-audit >/dev/null 2>&1; then
  for b in $BINS; do
    if [ -x "$next/bin/$b" ]; then
      ln -sfn "$next/bin/$b" "/usr/local/bin/$b"
    fi
  done
fi
