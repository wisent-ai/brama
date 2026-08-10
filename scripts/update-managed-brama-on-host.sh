#!/bin/sh
# Move the managed Brama service onto one exact repository revision.
#
# Run through `stado host install-helper` + `run-helper`: that channel passes no
# arguments, so the revision is pinned in this file and changes with a commit.
# Idempotent: an already-installed revision reports and exits without work.
set -eu

REVISION=9d3a259753895c0c35d592d4fcc0d860da8a6474
REPOSITORY=https://github.com/wisent-ai/brama.git
BUNDLE="$HOME/.stado/services/brama/current/darwin-arm"
WORK="$HOME/.stado/build-work/brama-managed"
UNIT=com.wisent.always-on.brama

PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

report() { printf '%s\n' "$1"; }

[ -d "$BUNDLE/bin" ] || { report "no managed brama bundle at $BUNDLE"; exit 1; }

stamp="$BUNDLE/bin/brama.revision"
if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$REVISION" ]; then
  report "already at $REVISION"
  exit 0
fi

cargo_bin=$(command -v cargo || true)
if [ -z "$cargo_bin" ]; then
  for candidate in "$HOME"/.rustup/toolchains/*/bin/cargo; do
    [ -x "$candidate" ] && cargo_bin="$candidate" && break
  done
fi
[ -n "$cargo_bin" ] || { report "no cargo toolchain on this host"; exit 1; }

mkdir -p "$(dirname "$WORK")"
if [ ! -d "$WORK/.git" ]; then
  rm -rf "$WORK"
  git clone --filter=blob:none --no-checkout "$REPOSITORY" "$WORK"
fi
git -C "$WORK" fetch --depth 1 origin "$REVISION"
git -C "$WORK" checkout --detach --force "$REVISION"

CARGO_TARGET_DIR="$WORK/target" "$cargo_bin" build --locked --release \
  --manifest-path "$WORK/Cargo.toml" --bin brama

built="$WORK/target/release/brama"
[ -x "$built" ] || { report "build produced no brama binary"; exit 1; }

backup="$BUNDLE/bin/brama.before-$REVISION"
[ -f "$backup" ] || cp -p "$BUNDLE/bin/brama" "$backup"
install -m 0755 "$built" "$BUNDLE/bin/brama"
printf '%s\n' "$REVISION" >"$stamp"

restart=deferred
if launchctl kickstart -k "system/$UNIT" >/dev/null 2>&1; then
  restart=kickstarted
elif sudo -n launchctl kickstart -k "system/$UNIT" >/dev/null 2>&1; then
  restart=kickstarted-with-sudo
fi

report "installed $REVISION backup=$backup restart=$restart"
