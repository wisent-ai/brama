if [ -x "$bundle_root/bin/brama" ] && [ -x "$bundle_root/bin/skarbiec-entitlements-router" ]; then
  default_brama_bin="$bundle_root/bin/brama"
  default_router_bin="$bundle_root/bin/skarbiec-entitlements-router"
  default_config_dir="${HOME:-/nonexistent}/.config/brama/trust-$installation"
  bundled_installation=1
else
  default_brama_bin=/usr/local/bin/brama
  default_router_bin=/usr/local/bin/skarbiec-entitlements-router
  default_config_dir=/etc/brama-skarbiec
  bundled_installation=0
fi

requested_runtime_dir=${BRAMA_RUNTIME_DIR:-}
requested_config_dir=${BRAMA_SKARBIEC_CONFIG_DIR:-}
requested_port_override=${BRAMA_PORT_OVERRIDE:-}
service_env_file=${BRAMA_SERVICE_ENV_FILE:-${HOME:-/nonexistent}/.config/brama/service.env}
if [ -f "$service_env_file" ]; then
  set -a
  . "$service_env_file"
  set +a
elif [ -n "${BRAMA_SERVICE_ENV_FILE:-}" ]; then
  printf '%s\n' "BRAMA_SERVICE_ENV_FILE is not a regular file: $service_env_file" >/dev/stderr
  false
fi
# The supervisor owns process-local coordinates. A service.env written for an
# older installation may still name its runtime directory or candidate port;
# sourcing it must not collapse a blue-green pair back onto one process state.
if [ -n "$requested_runtime_dir" ]; then
  BRAMA_RUNTIME_DIR=$requested_runtime_dir
  export BRAMA_RUNTIME_DIR
fi
if [ -n "$requested_port_override" ]; then
  BRAMA_PORT_OVERRIDE=$requested_port_override
  export BRAMA_PORT_OVERRIDE
fi
configured_config_dir=$requested_config_dir

# A versioned bundle carries its own executables and trust material; these must
# move together because registry.json binds capabilities to that exact binary.
# service.env may retain paths from an older digest after a Stado update.
if [ "$bundled_installation" -eq 1 ]; then
  BRAMA_BIN="$default_brama_bin"
  ENTITLEMENTS_ROUTER_BIN="$default_router_bin"
  BRAMA_SKARBIEC_CONFIG_DIR="$default_config_dir"
else
  if [ -x "$default_brama_bin" ]; then BRAMA_BIN="$default_brama_bin"; fi
  if [ -x "$default_router_bin" ]; then ENTITLEMENTS_ROUTER_BIN="$default_router_bin"; fi
  if [ -d "$default_config_dir" ]; then BRAMA_SKARBIEC_CONFIG_DIR="$default_config_dir"; fi
fi
BRAMA_BIN=${BRAMA_BIN:-"$default_brama_bin"}
ENTITLEMENTS_ROUTER_BIN=${ENTITLEMENTS_ROUTER_BIN:-"$default_router_bin"}
config_dir=${BRAMA_SKARBIEC_CONFIG_DIR:-"$default_config_dir"}
if [ "$bundled_installation" -eq 0 ] && [ ! -x "$ENTITLEMENTS_ROUTER_BIN" ]; then
  if [ -x "${HOME:-/nonexistent}/.stado/bin/skarbiec" ]; then
    ENTITLEMENTS_ROUTER_BIN="${HOME:-/nonexistent}/.stado/bin/skarbiec"
  else
    discovered_router=$(command -v skarbiec || true)
    if [ -n "$discovered_router" ] && [ -x "$discovered_router" ]; then
      ENTITLEMENTS_ROUTER_BIN="$discovered_router"
    fi
  fi
fi
if [ -n "${BRAMA_BIN_OVERRIDE:-}" ]; then
  [ -x "$BRAMA_BIN_OVERRIDE" ] || {
    printf '%s\n' "BRAMA_BIN_OVERRIDE is not executable: $BRAMA_BIN_OVERRIDE" >/dev/stderr
    false
  }
  BRAMA_BIN="$BRAMA_BIN_OVERRIDE"
  # A source-tree binary is not a system installation. Provision its generated
  # trust beside the user's service state unless the caller named another
  # directory; /etc is both shared with production and unwritable in a normal
  # development or Probierz journey.
  if [ -z "$requested_runtime_dir" ]; then
    unset BRAMA_RUNTIME_DIR
  fi
  if [ -z "$configured_config_dir" ]; then
    BRAMA_SKARBIEC_CONFIG_DIR="${HOME:-/nonexistent}/.config/brama/trust"
    config_dir="$BRAMA_SKARBIEC_CONFIG_DIR"
  fi
fi
PYTHON_BIN=${PYTHON_BIN:-python3}
command -v "$PYTHON_BIN" >/dev/null 2>&1 || {
  printf '%s\n' "PYTHON_BIN is not executable: $PYTHON_BIN" >/dev/stderr
  false
}
