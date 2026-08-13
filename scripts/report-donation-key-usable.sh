#!/usr/bin/env bash
# Report whether the configured donation recipient can actually be encrypted to.
#
# Naming a recipient the vault already uses is necessary but not sufficient:
# the gateway encrypts with gpg inside its own GNUPGHOME, and a key that is not
# in that keyring fails the write exactly like an unknown name would. That
# failure is logged without its reason, so it has to be checked here instead.
#
# Read-only: lists the key and encrypts a fixed throwaway string to /dev/null.
# Nothing is written to the vault and no secret is read.
set -u

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

SERVICE_ENV="$HOME/.config/brama/service.env"
recipient=$(/usr/bin/sed -n 's/^SKARBIEC_DONATION_RECIPIENT=//p' "$SERVICE_ENV" 2>/dev/null | /usr/bin/tail -1 | /usr/bin/tr -d '"')
[ -n "${recipient:-}" ] || recipient="brama-service"
gnupg=$(/usr/bin/sed -n 's/^BRAMA_GNUPG_HOME=//p' "$SERVICE_ENV" 2>/dev/null | /usr/bin/tail -1 | /usr/bin/tr -d '"')
[ -n "${gnupg:-}" ] || gnupg="$HOME/.gnupg"

echo "recipient: $recipient"
echo "keyring:   $gnupg"

echo
echo "=== key present in that keyring ==="
GNUPGHOME="$gnupg" /usr/bin/env gpg --batch --list-keys "$recipient" 2>&1 | /usr/bin/sed -n '/^pub\|^uid/p' | head -4
echo "(no lines above means the keyring has no such key)"

echo
echo "=== can the gateway encrypt to it ==="
if printf 'probe' | GNUPGHOME="$gnupg" /usr/bin/env gpg --batch --yes --trust-model always \
    --encrypt --recipient "$recipient" --output /dev/null 2>/tmp/donation-key-probe.err; then
  echo "encrypt: ok"
else
  echo "encrypt: FAILED"
  /usr/bin/head -3 /tmp/donation-key-probe.err
fi
/bin/rm -f /tmp/donation-key-probe.err


# The binary's default, tested beside the configured one so the difference is
# visible rather than asserted: this is the value every failed write used.
echo
echo "=== the binary's default, for comparison ==="
if printf 'probe' | GNUPGHOME="$gnupg" /usr/bin/env gpg --batch --yes --trust-model always \
    --encrypt --recipient "brama-service" --output /dev/null 2>/tmp/donation-default-probe.err; then
  echo "brama-service: encrypt ok"
else
  echo "brama-service: encrypt FAILED"
  /usr/bin/head -2 /tmp/donation-default-probe.err
fi
/bin/rm -f /tmp/donation-default-probe.err