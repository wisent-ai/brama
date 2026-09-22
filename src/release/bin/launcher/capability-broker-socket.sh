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
