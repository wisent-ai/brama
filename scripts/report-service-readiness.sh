#!/bin/sh
# Read-only readiness report for the Brama always-on unit on a fleet host.
#
# The unit dies inside start-with-skarbiec long before the gateway binds, and
# the only evidence left behind is one line of stdout in the launchd log. This
# reports what that launcher is about to require and what the host actually
# has, so the missing piece is named instead of guessed from another machine.
#
# It prints names, paths and presence only. The service env file carries
# credentials, so no value from it is ever printed.
set -eu

env_file="${BRAMA_SERVICE_ENV_FILE:-$HOME/.config/brama/service.env}"
bundle="$HOME/.stado/services/brama/current/darwin-arm"
launcher="$bundle/bin/start-with-skarbiec"
control_config="$HOME/.config/brama/control.json"

report_path() {
  if [ -e "$aim" ]
  then
    printf '%s\tpresent\t%s\n' "$label" "$aim"
  else
    printf '%s\tmissing\t%s\n' "$label" "$aim"
  fi
}

for pair in \
  "env_file:$env_file" \
  "launcher:$launcher" \
  "control_config:$control_config" \
  "bundle:$bundle"
do
  label="${pair%%:*}"
  aim="${pair#*:}"
  report_path
done

printf '%s\n' '--- variables the launcher requires ---'
if [ -f "$launcher" ]
then
  sed -n 's/^: *"${\([A-Za-z_][[:alnum:]_]*\):[?=].*/\1/p' "$launcher" | sort -u
else
  printf '%s\n' 'launcher-missing'
fi

printf '%s\n' '--- variables the env file defines (names only) ---'
if [ -f "$env_file" ]
then
  sed -n 's/^ *\(export \)\{0,\}\([A-Za-z_][[:alnum:]_]*\)=.*/\2/p' "$env_file" | sort -u
else
  printf '%s\n' 'env-file-missing'
fi

printf '%s\n' '--- candidate GnuPG homes ---'
for candidate in \
  "$HOME/.config/brama/gnupg" \
  "$HOME/.gnupg" \
  "/tmp/brama-skarbiec/gnupg"
do
  label=gnupg
  aim="$candidate"
  report_path
done

# Paths only. A launcher that refuses to start because a file it was pointed
# at does not exist is indistinguishable, from another machine, from one that
# was never pointed anywhere -- so the declared path and its presence are the
# report, and nothing that could carry a credential is read.
printf '%s\n' '--- declared paths and whether they exist ---'
if [ -f "$env_file" ]
then
  for name in \
    BRAMA_BIN \
    BRAMA_CONTROL_CONFIG \
    BRAMA_GNUPG_HOME \
    BRAMA_INFERENCE_ROUTES_FILE \
    BRAMA_SKARBIEC_CONFIG_DIR \
    ENTITLEMENTS_ROUTER_BIN \
    PYTHON_BIN \
    SKARBIEC_VAULT_FILE
  do
    value="$(sed -n "s/^ *\(export \)*$name=//p" "$env_file" | tr -d \" | tr -d \')"
    if [ -z "$value" ]
    then
      printf '%s\tundeclared\t-\n' "$name"
    else
      label="$name"
      aim="$value"
      report_path
    fi
  done
fi

printf '%s\n' '--- launchd state ---'
launchctl print system/com.wisent.always-on.brama | sed -n 's/^ *\(state\|last exit code\|path\|program\) *= *\(.*\)/\1 \2/p' || printf '%s\n' 'launchctl-print-unavailable'

# `stado service logs` tails the unit's stdout. A launcher that dies before it
# prints anything leaves its reason in the error file instead, which is why the
# stdout tail can stay frozen on a week-old line while the unit keeps failing.
printf '%s\n' '--- unit log files ---'
for log in \
  "$HOME/.stado/logs/brama-always-on.out" \
  "$HOME/.stado/logs/brama-always-on.err"
do
  label=log
  aim="$log"
  report_path
  if [ -f "$log" ]
  then
    printf 'modified %s\n' "$(date -r "$log" -u)"
    tail -n 100 "$log"
  fi
done

printf '%s\n' '--- unit definition ---'
plist=/Library/LaunchDaemons/com.wisent.always-on.brama.plist
label=plist
aim="$plist"
report_path
if [ -f "$plist" ]
then
  sed -n 's/.*<key>\(.*\)<\/key>.*/key \1/p;s/.*<string>\(.*\)<\/string>.*/  \1/p' "$plist"
fi

# A process that dies with nothing in either log did not choose to stop. macOS
# records that separately, and without it the launcher's last line looks like
# a successful start followed by a mystery.
printf '%s\n' '--- recent crash reports ---'
reports="$HOME/Library/Logs/DiagnosticReports"
label=reports
aim="$reports"
report_path
if [ -d "$reports" ]
then
  newest="$(ls -t "$reports" | sed -n '/^brama/p' | while read -r name; do printf '%s\n' "$name"; break; done)"
  if [ -z "$newest" ]
  then
    printf '%s\n' 'no-brama-crash-report'
  else
    printf 'newest %s\n' "$newest"
    sed -n '/exception/p;/termination/p;/signal/p;/"reason"/p' "$reports/$newest" | tr -s ' ' | cut -c-"$(printf '%s' 240)"
  fi
fi

# Under `set -e` a failing command exits the launcher with no message of its
# own, so the last line it printed is the only marker of how far it got. The
# code after that marker is the suspect list.
printf '%s\n' '--- launcher after its last printed line ---'
if [ -f "$launcher" ]
then
  sed -n '/serving the fleet/,$p' "$launcher"
fi

# Skarbiec issues a capability only for a resource its routes table maps to a
# vault coordinate. With no table the gateway starts with no provider it may
# authenticate to and the first alias that needs one ends the process, which
# reads as a model-alias fault and is not one.
printf '%s\n' '--- capability routing ---'
vault="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
label=vault
aim="$vault"
report_path
label=routes
aim="$(dirname "$vault")/capability-routes.json"
report_path
if [ -n "${SKARBIEC_CAPABILITY_ROUTES_FILE:-}" ]
then
  label=routes_env
  aim="$SKARBIEC_CAPABILITY_ROUTES_FILE"
  report_path
fi

# Item ids only. The router writes this list itself at every start; the values
# behind the ids stay in the vault.
printf '%s\n' '--- item ids the router listed at last start ---'
subscriptions=/tmp/brama-skarbiec/subscriptions.json
label=subscriptions
aim="$subscriptions"
report_path
if [ -f "$subscriptions" ]
then
  sed -n 's/.*"id":"\([^"]*\)".*/\1/p' "$subscriptions" | sort -u
  tr ',' '\n' < "$subscriptions" | sed -n 's/.*"id": *"\([^"]*\)".*/\1/p' | sort -u
fi

# Coordinates, not secrets: this table names vault items and fields, which is
# the same class as the ids above. Its shape is the thing in question when a
# present file still maps nothing.
printf '%s\n' '--- capability routes table ---'
routes_file="$(dirname "$vault")/capability-routes.json"
if [ -f "$routes_file" ]
then
  wc -c < "$routes_file"
  cat "$routes_file"
fi

# A route needs one item and one field. The item id is already the operator's
# choice -- the vault names it exactly as the resource -- but the field name is
# not visible from the resource, and `inspect-vault` is the one reader that
# gives names without values.
printf '%s\n' '--- vault item metadata (nonsecret) ---'
stado_bin="$HOME/.stado/bin/stado"
if [ -x "$stado_bin" ] && [ -f "$vault" ]
then
  "${PYTHON_BIN:-python3}" "$HOME/.stado/bin/brama-readiness-fields" --vault "$stado_bin" "$vault"
else
  printf '%s\n' 'stado-or-vault-missing'
fi

printf '%s\n' '--- brama-runtime policy rules ---'
policy="${BRAMA_SKARBIEC_CONFIG_DIR:-}/policy.json"
if [ ! -f "$policy" ]
then
  policy="$bundle/etc/brama-skarbiec/policy.json"
fi
label=policy
aim="$policy"
report_path
if [ -f "$policy" ]
then
  cat "$policy"
fi

# Field names, never field values. A route needs one item and one field name,
# and picking the field is the difference between handing a purpose the key it
# was authorised for and handing it a neighbouring one.
printf '%s\n' '--- field names of the resources the router lists ---'
if [ -f "$subscriptions" ]
then
  "${PYTHON_BIN:-python3}" "$HOME/.stado/bin/brama-readiness-fields" "$subscriptions"
fi

printf '%s\n' '--- bundle contents ---'
if [ -d "$bundle" ]
then
  ls "$bundle/bin" "$bundle/etc" || true
else
  printf '%s\n' 'bundle-missing'
fi
