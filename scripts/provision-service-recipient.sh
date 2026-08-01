#!/bin/sh
set -eu
umask u=rwx,go=

identity=${BRAMA_SKARBIEC_RECIPIENT_IDENTITY:-brama-rtx@wisent.local}
gnupg_home=${BRAMA_SKARBIEC_GNUPG_HOME:-"$HOME/.stado/brama-gnupg"}
mkdir -p "$gnupg_home"
chmod u=rwx,go= "$gnupg_home"

if ! gpg --batch --quiet --homedir "$gnupg_home" --list-secret-keys "$identity" >/dev/null; then
  gpg --batch --homedir "$gnupg_home" --generate-key <<EOF
Key-Type: eddsa
Key-Curve: ed25519
Key-Usage: cert sign
Subkey-Type: ecdh
Subkey-Curve: cv25519
Subkey-Usage: encrypt
Name-Real: Brama RTX service
Name-Email: $identity
%no-protection
%commit
EOF
fi

gpg --batch --homedir "$gnupg_home" --armor --export "$identity"
