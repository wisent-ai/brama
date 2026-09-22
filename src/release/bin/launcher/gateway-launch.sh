# The host's Skarbiec process owns redemption. Prove that this client and that
# listener agree on the vault, capability state and routes before Brama starts.
# Never replace its socket or give a gateway generation its own broker.
"$ENTITLEMENTS_ROUTER_BIN" capability-status --socket "$SKARBIEC_CAP_SOCKET" >&2

# Administrative journeys use the same shared authority and workload identity.
# Exec preserves the process the caller supervises without a shell or reaper.
if [ "${1:-}" = "--exec" ]; then
  shift
  [ "$#" -gt 0 ] || {
    printf '%s\n' 'start-with-skarbiec.sh --exec requires a command' >/dev/stderr
    exit 2
  }
  exec "$@"
fi
if [ "$#" -gt 0 ]; then
  exec "$BRAMA_BIN" "$@"
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

# The supervisor owns Brama's actual PID; Skarbiec has its own service owner.
exec "$BRAMA_BIN" serve --port "$brama_port"
