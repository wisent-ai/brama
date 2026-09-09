# One runtime directory per installation, not one for the machine. The broker
# socket lives here, and the broker answers with the trust material of the
# bundle that started it. A single shared path meant a gateway from a new
# bundle redeemed against a broker left running by an older one, whose registry
# describes a different workload -- so the authority issued the capability and
# the broker then denied redeeming it, which is a hard failure to read because
# both halves are working exactly as told.
runtime_dir=${BRAMA_RUNTIME_DIR:-"${HOME:-/nonexistent}/.stado/run/brama-skarbiec-$installation"}
socket_dir="$runtime_dir/socket"
gnupg_dir="$runtime_dir/gnupg"
worm_dir="$runtime_dir/worm"
mkdir -p "$runtime_dir" "$socket_dir" "$gnupg_dir" "$worm_dir"
chmod u=rwx,go= "$runtime_dir" "$gnupg_dir" "$worm_dir"
chmod u=rwx,g=rx,o= "$socket_dir"

: "${BRAMA_GNUPG_HOME:?BRAMA_GNUPG_HOME is required}"
[ -d "$BRAMA_GNUPG_HOME" ] || {
  printf '%s\n' "BRAMA_GNUPG_HOME is not a directory" >/dev/stderr
  false
}
export GNUPGHOME="$BRAMA_GNUPG_HOME"

# This service has its own identity, and the fleet already keeps it: the vault
# item `brama-service` carries the private half for exactly this purpose. Import
# it here, because everything below decrypts with it, and without it the
# entitlements router fails with "No secret key" and no indication that the key
# was ever meant to be somewhere.
if [ -x "$HOME/.stado/bin/stado" ]; then
  stado_bin="$HOME/.stado/bin/stado"
else
  stado_bin="$(command -v stado || true)"
fi
# The service key must match a recipient of the encrypted Brama items. An
# unrelated private key is not enough: operator keyrings can contain other
# identities while still being unable to decrypt the model-router records.
service_identity_present=
for recipient_key in 7E0441E08C5CEAAC 6C6746F4AB546CB4 C30E7BF28DDE114E; do
  if gpg --batch --list-secret-keys "$recipient_key" >/dev/null 2>&1; then
    service_identity_present=1
    break
  fi
done
if [ -n "$stado_bin" ] && [ -z "$service_identity_present" ]; then
  # An explicitly configured endpoint belongs to this Brama installation and
  # wins over the fleet default. The fleet can run another Skarbiec instance
  # for host management; using that endpoint here couples Brama startup to an
  # unrelated keyring and can leave a healthy listener unable to decrypt the
  # Brama service identity.
  if [ -n "${WC_AGENT_SKARBIEC_URL:-}" ]; then
    agent_skarbiec_url="$WC_AGENT_SKARBIEC_URL"
  else
    fleet_stado_config=${BRAMA_FLEET_STADO_CONFIG:-"${HOME:-/nonexistent}/.config/stado/config.json"}
    agent_skarbiec_url="$(
      STADO_CONFIG="$fleet_stado_config" "$stado_bin" config show \
        | "$PYTHON_BIN" -c '
import json
import sys
value = json.load(sys.stdin).get("resolved", {}).get("agent_skarbiec_url")
if not isinstance(value, str) or not value:
    raise SystemExit("fleet Stado config has no agent_skarbiec_url")
sys.stdout.write(value)
'
    )"
  fi
  service_key="$gnupg_dir/brama-service.key"
  rm -f "$service_key"
  read_attempt=1
  read_attempts=${BRAMA_SKARBIEC_READ_ATTEMPTS:-3}
  while ! ( umask 077
    WC_AGENT_SKARBIEC_URL="$agent_skarbiec_url" \
      STADO_CONFIG=${BRAMA_SKARBIEC_STADO_CONFIG:-"${HOME:-/nonexistent}/.config/stado/brama-service.json"} \
      "$stado_bin" secrets get brama-service --field gpg_private_key > "$service_key" )
  do
    rm -f "$service_key"
    if [ "$read_attempt" -ge "$read_attempts" ]; then
      printf '%s\n' 'cannot read this service identity from Skarbiec (brama-service.gpg_private_key)' >/dev/stderr
      false
    fi
    printf '%s\n' "Skarbiec could not return the Brama identity; retrying ($read_attempt/$read_attempts)" >/dev/stderr
    read_attempt=$((read_attempt + 1))
    sleep 2
  done
  gpg --batch --quiet --import "$service_key" || {
    rm -f "$service_key"
    printf '%s\n' 'the service identity from Skarbiec did not import' >/dev/stderr
    false
  }
  rm -f "$service_key"
fi
