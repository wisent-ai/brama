

"$ENTITLEMENTS_ROUTER_BIN" capability-serve &
broker_pid=$!
owner_pid=$$
(
  while kill -0 "$owner_pid" 2>/dev/null; do
    sleep 1
  done
  kill "$broker_pid" 2>/dev/null || true
) &
broker_reaper_pid=$!
trap 'kill "$broker_pid" "$broker_reaper_pid" 2>/dev/null || true' EXIT INT TERM
attempt=0
while [ ! -S "$SKARBIEC_CAP_SOCKET" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 100 ]; then
    echo 'Skarbiec capability broker did not create its socket' >&2
    exit 1
  fi
  sleep 0.05
done

# The same trust, routes and short-lived capabilities serve administrative CLI
# journeys and real product tests. Keeping that setup here prevents `brama
# onboard` from silently falling back to an unconfigured in-process router while
# the service itself is healthy. `--exec` runs a named program inside the exact
# launcher environment while trust remains pinned to BRAMA_BIN_OVERRIDE; this is
# how Cargo's real-binary journeys run without teaching the service launcher
# about Cargo. A command exits through the trap above, so its temporary broker is
# removed; the long-running service path below still uses `exec`.
if [ "${1:-}" = "--exec" ]; then
  shift
  [ "$#" -gt 0 ] || {
    printf '%s\n' 'start-with-skarbiec.sh --exec requires a command' >/dev/stderr
    exit 2
  }
  "$@"
  exit $?
fi
if [ "$#" -gt 0 ]; then
  "$BRAMA_BIN" "$@"
  exit $?
fi


# Releases before this launcher handed the gateway to a child shell. Stopping
# the launchd job therefore left that child alive on its port, and every newer
# release was quarantined even though its own process model was correct. Retire
# only an exact stale managed executable: never kill an arbitrary listener.
brama_port=${BRAMA_PORT_OVERRIDE:-${PORT:-8080}}
if [ -x /usr/sbin/lsof ]; then
  for stale_pid in $(/usr/sbin/lsof -nP -tiTCP:"$brama_port" -sTCP:LISTEN 2>/dev/null || true); do
    stale_bin=$(ps -p "$stale_pid" -o comm= 2>/dev/null || true)
    case "$stale_bin" in
      "${HOME:-/nonexistent}/.stado/services/brama/sha256-"*/darwin-arm/bin/brama|\
      "${HOME:-/nonexistent}/.stado/services/brama/sha256-"*/darwin-arm64/bin/brama|\
      "${HOME:-/nonexistent}/.stado/services/brama/releases/"*/darwin-arm64/bin/brama)
        if [ "$(realpath "$stale_bin")" != "$(realpath "$BRAMA_BIN")" ]; then
          printf '%s\n' "retiring stale managed Brama process $stale_pid from $stale_bin" >/dev/stderr
          kill "$stale_pid"
          attempt=0
          while kill -0 "$stale_pid" 2>/dev/null; do
            attempt=$((attempt + 1))
            if [ "$attempt" -ge 100 ]; then
              printf '%s\n' "stale managed Brama process $stale_pid did not stop" >/dev/stderr
              exit 1
            fi
            sleep 0.05
          done
        fi
        ;;
    esac
  done
fi

# `exec` keeps the gateway at the PID the supervisor owns. The broker reaper
# above watches that same PID across exec and ends the generation's broker when
# the gateway exits, so rollback cannot leave a candidate authority behind.
exec "$BRAMA_BIN" serve --port "$brama_port"
