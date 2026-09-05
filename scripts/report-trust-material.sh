#!/usr/bin/env bash
# Report whether this installation can re-sign its capability policy safely.
#
# The policy that decides which subscriptions the runtime may spend is signed at
# provisioning time, so adding a subscription means re-signing it. That is only
# safe when the workload's proof key survives: the registry re-pins path, digest
# and account freely, but a new proof key needs a fresh vault grant, and the
# service cannot authorise one because the vault is encrypted to the owner.
#
# `provision-skarbiec-trust.sh` refuses to run twice without --force for exactly
# that reason, while also exporting BRAMA_PROOF_KEY_FILE so a carried key is
# reused. Whether that carried key exists is therefore the whole question.
#
# Read-only. Prints paths, sizes and counts; no key material.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

CARRIED="$HOME/.config/brama/brama-proof.key"
# The launcher's default trust directory on a host is ~/.config/brama/trust,
# not the bundle's etc/: the bundle only carries the subscriptions manifest.
BUNDLE="$HOME/.stado/services/brama/current/darwin-arm"
CONFIG="${BRAMA_SKARBIEC_CONFIG_DIR:-$HOME/.config/brama/trust}"
MANIFEST_IN_BUNDLE="$BUNDLE/etc/brama-skarbiec/subscriptions.json"
echo "=== bundle ==="
echo "root:      $BUNDLE"
[ -d "$BUNDLE" ] && /bin/ls -1 "$BUNDLE" | head -6 | /usr/bin/sed 's/^/  /' || echo "  absent"

echo
echo "=== provisioning script ==="
for candidate in "$BUNDLE/bin/provision-skarbiec-trust.sh" "$BUNDLE/libexec/provision-skarbiec-trust.sh" "$BUNDLE/provision-skarbiec-trust.sh"; do
  [ -f "$candidate" ] && echo "  present: $candidate"
done
echo "  (no line means the bundle ships no provisioning script)"

echo
echo "=== config dir ==="
echo "path: $CONFIG"
if [ -d "$CONFIG" ]; then
  for name in policy.json policy.sig registry.json registry.sig trust.json brama-proof.key subscriptions.json worm-receipt; do
    if [ -e "$CONFIG/$name" ]; then
      /bin/ls -l "$CONFIG/$name" | /usr/bin/awk '{print "  "$9"  "$5" bytes"}'
    else
      echo "  $name  ABSENT"
    fi
  done
  echo "  subscriptions declared: $(/usr/bin/grep -c '"id"' "$CONFIG/subscriptions.json" 2>/dev/null || echo '?')"
else
  echo "  absent"
fi

echo
echo "=== carried proof key ==="
if [ -f "$CARRIED" ]; then
  /bin/ls -l "$CARRIED" | /usr/bin/awk '{print "  present: "$9"  "$5" bytes"}'
  echo "  re-signing reuses this identity"
else
  echo "  ABSENT: $CARRIED"
  echo "  re-signing would mint a new identity and strand the vault grant"
fi
