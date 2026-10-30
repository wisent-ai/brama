# Whose public keys donated subscriptions are encrypted to is the decision of
# the installation that runs Brama, not of the release: the archive ships no
# key. The installation declares them once in an armored file,
# `~/.config/brama/recipient-public-keys.asc` or the file
# BRAMA_DONATION_RECIPIENT_KEYS names, and each new installation directory is
# seeded from it, so a release update - which lands under a new digest
# directory - keeps the recipients without anyone noticing that a directory
# moved. A directory that already holds its keys keeps them.
#
# An installation that declares none is refused here, by name: started without
# recipients, a donation could not be recovered, and the gateway would fail at
# the first one instead.
declared_recipients=${BRAMA_DONATION_RECIPIENT_KEYS:-"${HOME:-/nonexistent}/.config/brama/recipient-public-keys.asc"}
if [ ! -f "$config_dir/recipient-public-keys.asc" ]; then
  [ -f "$declared_recipients" ] || {
    printf '%s\n' "no donation recipient keys: $config_dir/recipient-public-keys.asc is absent and $declared_recipients is not a file; put the armored public keys donated subscriptions are encrypted to there, or name the file with BRAMA_DONATION_RECIPIENT_KEYS" >/dev/stderr
    false
  }
  mkdir -p "$config_dir"
  chmod u=rwx,go= "$config_dir"
  cp "$declared_recipients" "$config_dir/recipient-public-keys.asc"
  printf '%s\n' "seeded recipient-public-keys.asc into $config_dir from $declared_recipients" >/dev/stderr
fi
unset declared_recipients
