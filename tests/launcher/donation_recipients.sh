#!/usr/bin/env bash
# Real test of the launcher stage that seeds an installation's donation
# recipient keys (src/release/bin/launcher/donation-recipients.sh), sourced
# the way start-with-skarbiec sources it, in this run's own HOME and
# installation directory:
#   * the release ships no recipient key, so an installation that declares
#     none is refused naming both paths and nothing is written;
#   * the keys declared in ~/.config/brama/recipient-public-keys.asc seed a new
#     installation directory, owner-only, byte for byte;
#   * BRAMA_DONATION_RECIPIENT_KEYS names another declaration;
#   * an installation directory that already holds its keys keeps them.
# Every step and its outcome go to the run's report.txt; a failed check stops
# the run after naming itself there.
#
# Usage: tests/launcher/donation_recipients.sh
set -eu
cd "$(dirname "$0")/../.."
STAGE="$PWD/src/release/bin/launcher/donation-recipients.sh"
RUN="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ROOT="$PWD/target/real-tests/launcher/donation-recipients-$RUN"
REPORT="$ROOT/report.txt"
mkdir -p "$ROOT"
echo "revision: $(git rev-parse HEAD 2>/dev/null || echo exported)" >"$REPORT"

fail() {
  echo "FAIL: $1" | tee -a "$REPORT" >/dev/stderr
  false
}
ok() {
  echo "ok: $1" >>"$REPORT"
}

[ ! -e "$PWD/src/release/etc/brama-skarbiec/recipient-public-keys.asc" ] \
  || fail "the release still ships a recipient key"
ok "the release ships no recipient key"

# seed HOME CONFIG_DIR [DECLARED]: source the stage as the launcher does;
# answers the stage's exit status, its stderr in $ROOT/stderr.
seed() {
  (
    HOME=$1
    config_dir=$2
    if [ -n "${3:-}" ]; then BRAMA_DONATION_RECIPIENT_KEYS=$3; else unset BRAMA_DONATION_RECIPIENT_KEYS; fi
    . "$STAGE"
  ) 2>"$ROOT/stderr"
}

home="$ROOT/none/home"
config="$ROOT/none/trust"
mkdir -p "$home"
if seed "$home" "$config"; then fail "an installation that declares no keys started"; fi
grep -q "^no donation recipient keys: $config/recipient-public-keys.asc is absent and $home/.config/brama/recipient-public-keys.asc is not a file" "$ROOT/stderr" \
  || fail "the refusal does not name both paths: $(cat "$ROOT/stderr")"
[ ! -e "$config" ] || fail "a refused start wrote $config"
ok "no declaration is refused naming both paths"

home="$ROOT/declared/home"
config="$ROOT/declared/trust"
mkdir -p "$home/.config/brama"
printf '%s\n' "-----BEGIN PGP PUBLIC KEY BLOCK-----" "example declared key" "-----END PGP PUBLIC KEY BLOCK-----" \
  >"$home/.config/brama/recipient-public-keys.asc"
seed "$home" "$config" || fail "a declared key was refused: $(cat "$ROOT/stderr")"
cmp -s "$home/.config/brama/recipient-public-keys.asc" "$config/recipient-public-keys.asc" \
  || fail "the installation was not seeded with the declared keys"
case "$(ls -ld "$config")" in drwx------*) ;; *) fail "the installation directory is not owner-only: $(ls -ld "$config")" ;; esac
grep -q "seeded recipient-public-keys.asc into $config from $home/.config/brama/recipient-public-keys.asc" "$ROOT/stderr" \
  || fail "the seed is not announced: $(cat "$ROOT/stderr")"
ok "the declared keys seed a new installation, owner-only"

named="$ROOT/named.asc"
printf '%s\n' "example named key" >"$named"
config="$ROOT/named/trust"
seed "$ROOT/none/home" "$config" "$named" || fail "BRAMA_DONATION_RECIPIENT_KEYS was refused: $(cat "$ROOT/stderr")"
cmp -s "$named" "$config/recipient-public-keys.asc" || fail "BRAMA_DONATION_RECIPIENT_KEYS did not seed the installation"
ok "BRAMA_DONATION_RECIPIENT_KEYS names the declaration"

printf '%s\n' "example replaced key" >"$home/.config/brama/recipient-public-keys.asc"
seed "$home" "$ROOT/declared/trust" || fail "an installation holding keys was refused"
grep -q "example declared key" "$ROOT/declared/trust/recipient-public-keys.asc" \
  || fail "an installation's own keys were replaced"
ok "an installation that holds its keys keeps them"

echo "passed: $REPORT"
