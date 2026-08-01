#!/bin/sh
set -eu

name=skarbiec-capability-clients
gid=$(id -g)
existing=$(getent group "$name" || true)

if [ -n "$existing" ]; then
  actual=$(printf '%s\n' "$existing" | cut -d: -f3)
  [ "$actual" = "$gid" ] || {
    printf '%s\n' "$name has gid $actual; service requires $gid" >/dev/stderr
    false
  }
  printf '%s\n' "$name:$gid"
  exit
fi

sudo -n /usr/sbin/groupadd --non-unique --gid "$gid" "$name"
getent group "$name"
