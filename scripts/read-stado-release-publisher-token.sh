#!/bin/sh
set -eu
: "${STADO_BIN:?STADO_BIN is required}"

exec "$STADO_BIN" secrets get brama-release-publisher --field token
