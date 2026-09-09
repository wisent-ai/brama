# Brama resolves a bearer it does not recognise by asking Skarbiec what it is,
# instead of refusing everything absent from the table this launcher builds at
# start. That call needs an identity of its own: one grant carrying `introspect`
# on `tokens`, provisioned like every other consumer here.
#
# Missing grant is not fatal. Without it the gateway behaves exactly as it did
# before -- a bearer outside the table is refused -- and says so once at start
# rather than leaving every such refusal to look like a bad key.
BRAMA_SKARBIEC_CONSUMER=${BRAMA_SKARBIEC_CONSUMER:-brama-token-introspector}
BRAMA_SKARBIEC_TOKEN_FILE=${BRAMA_SKARBIEC_TOKEN_FILE:-"${HOME:-/nonexistent}/.stado/brama-token-introspector-skarbiec-token"}
if [ -r "$BRAMA_SKARBIEC_TOKEN_FILE" ]; then
  export BRAMA_SKARBIEC_CONSUMER
  export BRAMA_SKARBIEC_TOKEN_FILE
else
  printf '%s\n' "no introspection grant at $BRAMA_SKARBIEC_TOKEN_FILE; a bearer this start did not read will be refused" >/dev/stderr
  unset BRAMA_SKARBIEC_CONSUMER
  unset BRAMA_SKARBIEC_TOKEN_FILE
fi

# The service directory is host-relative: Stado adapters on this host route to
# Brama's loopback listener, while public ingress terminates before forwarding
# locally. Ignore stale deployment variables from the superseded fleet-wide
# plaintext listener.
unset BRAMA_BIND_ADDRESS
unset BRAMA_ENCRYPTED_PEER_IPS
if [ -e "$SKARBIEC_CAP_SOCKET" ] || [ -L "$SKARBIEC_CAP_SOCKET" ]; then
  [ -S "$SKARBIEC_CAP_SOCKET" ] && [ ! -L "$SKARBIEC_CAP_SOCKET" ] || {
    printf '%s\n' "unsafe stale capability socket: $SKARBIEC_CAP_SOCKET" >/dev/stderr
    false
  }
  # An owner here used to end the start, and nothing ever cleared it: the
  # launcher is the only thing that creates this broker, so a live owner is a
  # leftover from an earlier start -- and until the trap above was fixed, every
  # failed start produced one. Refusing made the first failure permanent.
  #
  # End it, but only when it really is this installation's broker. Anything
  # else holding the path is a situation this script must not resolve by
  # killing a process it cannot identify, so that still refuses.
  lsof_bin=$(command -v lsof || true)
  if [ -n "$lsof_bin" ]; then
    for owner in $("$lsof_bin" -t -- "$SKARBIEC_CAP_SOCKET" || true); do
      owner_command=$(ps -p "$owner" -o comm= || true)
      case "$owner_command" in
        *skarbiec-entitlements-router|*/skarbiec)
          printf '%s\n' "ending a leftover capability broker: $owner" >/dev/stderr
          kill "$owner" || true
          ;;
        "")
          ;;
        *)
          printf '%s\n' "capability socket is held by $owner_command ($owner), which this launcher did not start" >/dev/stderr
          false
          ;;
      esac
    done
  fi
  rm -f -- "$SKARBIEC_CAP_SOCKET"
fi
