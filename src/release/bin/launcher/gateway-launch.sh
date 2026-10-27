# The host's Skarbiec process owns redemption. Prove that this client and that
# listener agree on the vault, capability state and routes before Brama starts.
# Never replace its socket or give a gateway generation its own broker.
"$ENTITLEMENTS_ROUTER_BIN" capability status --socket "$SKARBIEC_CAP_SOCKET" >&2

# Administrative journeys use the same shared authority and workload identity.
# Exec preserves the process the caller supervises without a shell or reaper.
if [ -n "${1+set}" ] && [ "$1" = "--exec" ]; then
  shift
  if [ -z "${1+set}" ]; then
    printf '%s\n' 'start-with-skarbiec.sh --exec requires a command; nothing was started' >/dev/stderr
    false
    exit
  fi
  exec "$@"
fi
if [ -n "${1+set}" ]; then
  exec "$BRAMA_BIN" "$@"
fi


# Releases before this launcher handed the gateway to a child shell. Stopping
# the launchd job therefore left that child alive on its port, and every newer
# release was quarantined even though its own process model was correct. Retire
# only an exact stale managed executable: never kill an arbitrary listener.
# `brama_port` was resolved by bundle-coordinates.sh from the declaration.
if [ -x /usr/sbin/lsof ]; then
  # lsof answers nothing, with a failing status, when no process listens.
  for stale_pid in $(/usr/sbin/lsof -nP -tiTCP:"$brama_port" -sTCP:LISTEN 2>/dev/null); do
    # macOS prints `comm` as the bare program name (`brama`), which no managed
    # path pattern below matches, so no stale gateway was ever retired. The
    # executable's path is the process's first text mapping, as lsof names it.
    stale_bin=$(/usr/sbin/lsof -a -p "$stale_pid" -d txt -Fn 2>/dev/null | /usr/bin/sed -n '/^n/{s/^n//;p;q;}')
    case "$stale_bin" in
      "$HOME/.stado/services/brama/sha256-"*/darwin-arm/bin/brama|\
      "$HOME/.stado/services/brama/sha256-"*/darwin-arm64/bin/brama|\
      "$HOME/.stado/services/brama/releases/"*/darwin-arm64/bin/brama)
        if [ "$(realpath "$stale_bin")" != "$(realpath "$BRAMA_BIN")" ]; then
          printf '%s\n' "retiring stale managed Brama process $stale_pid from $stale_bin" >/dev/stderr
          # No wait for it to go: if it still holds the port when Brama binds,
          # Brama's own bind error names the port and the supervisor restarts.
          kill "$stale_pid"
        fi
        ;;
    esac
  done
fi

# The supervisor owns Brama's actual PID; Skarbiec has its own service owner.
exec "$BRAMA_BIN" serve --port "$brama_port"
