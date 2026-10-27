# Brama resolves a bearer it does not recognise by asking Skarbiec what it is,
# instead of refusing everything absent from the table this launcher builds at
# start. That call needs an identity of its own: one grant carrying `introspect`
# on `tokens`, provisioned like every other consumer here.
#
# The consumer is Brama's declared introspector and its token file sits in the
# running user's Stado home, where Stado provisions it; an operator may name
# either with BRAMA_SKARBIEC_CONSUMER or BRAMA_SKARBIEC_TOKEN_FILE.
#
# Missing grant is not fatal. Without it the gateway behaves exactly as it did
# before -- a bearer outside the table is refused -- and says so once at start,
# naming why, rather than leaving every such refusal to look like a bad key.
if [ -z "${BRAMA_SKARBIEC_CONSUMER+set}" ]; then
  BRAMA_SKARBIEC_CONSUMER=brama-token-introspector
fi
introspection_unavailable=
if [ -z "${BRAMA_SKARBIEC_TOKEN_FILE+set}" ]; then
  if [ -n "${HOME+set}" ] && [ -n "$HOME" ]; then
    BRAMA_SKARBIEC_TOKEN_FILE="$HOME/.stado/brama-token-introspector-skarbiec-token"
  else
    introspection_unavailable="HOME is not set, so the introspection grant under the Stado home cannot be found; set BRAMA_SKARBIEC_TOKEN_FILE"
  fi
fi
if [ -z "$introspection_unavailable" ] && [ ! -r "$BRAMA_SKARBIEC_TOKEN_FILE" ]; then
  introspection_unavailable="no readable introspection grant at $BRAMA_SKARBIEC_TOKEN_FILE"
fi
if [ -z "$introspection_unavailable" ]; then
  export BRAMA_SKARBIEC_CONSUMER
  export BRAMA_SKARBIEC_TOKEN_FILE
else
  printf '%s\n' "$introspection_unavailable; a bearer this start did not read will be refused" >&2
  unset BRAMA_SKARBIEC_CONSUMER
  unset BRAMA_SKARBIEC_TOKEN_FILE
fi
unset introspection_unavailable

# The service directory is host-relative: Stado adapters on this host route to
# Brama's loopback listener, while public ingress terminates before forwarding
# locally. Ignore stale deployment variables from the superseded fleet-wide
# plaintext listener.
unset BRAMA_BIND_ADDRESS
unset BRAMA_ENCRYPTED_PEER_IPS
