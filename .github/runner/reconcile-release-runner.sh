#!/bin/bash
set -euo pipefail
umask 077

export HOME=/Users/charles
export PATH="$HOME/.stado/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

root="${BRAMA_RUNNER_ROOT:-$HOME/.stado/actions-runner-brama-release}"
runner_name="${BRAMA_RUNNER_NAME:-charless-mac-mini-stado-release}"
runner_labels="${BRAMA_RUNNER_LABELS:-stado-release}"
runner_url="${BRAMA_RUNNER_URL:-https://github.com/wisent-ai/brama}"
# v2.336.0 regresses process startup on macOS arm64: actions/runner#4570.
runner_version="2.335.1"
runner_sha256="e1a9bc7a3661e06fa0b129d15c2064fe65dc81a431001d8958a9db1409b73769"
runner_layout="3"
runner_dir="$root/vendor-$runner_version-layout-$runner_layout"
token_file="${BRAMA_RUNNER_REGISTRATION_TOKEN_FILE:-$root/.registration-token}"
archive="$root/actions-runner-osx-arm64-$runner_version.tar.gz"
incoming="$runner_dir.incoming"

/bin/mkdir -p "$root"
if [[ ! -x "$runner_dir/run.sh" || ! -x "$runner_dir/config.sh" ]]; then
  /bin/rm -rf "$incoming"
  /bin/mkdir -p "$incoming"
  /usr/bin/curl --fail --silent --show-error --location --max-time 120 \
    "https://github.com/actions/runner/releases/download/v$runner_version/actions-runner-osx-arm64-$runner_version.tar.gz" \
    --output "$archive"
  actual="$(/usr/bin/shasum -a 256 "$archive")"
  actual="${actual%% *}"
  if [[ "$actual" != "$runner_sha256" ]]; then
    printf 'GitHub Actions runner digest mismatch: expected %s, got %s\n' \
      "$runner_sha256" "$actual" >&2
    exit 1
  fi
  (
    umask 022
    /usr/bin/tar -xzf "$archive" -C "$incoming"
  )
  /bin/chmod -R u+rwX,go+rX "$incoming"
  /usr/bin/codesign --remove-signature "$incoming/bin/Runner.Listener"
  /usr/bin/codesign --remove-signature "$incoming/bin/Runner.Worker"
  [[ -x "$incoming/run.sh" && -x "$incoming/config.sh" ]]
  /bin/rm -rf "$runner_dir"
  /bin/mv "$incoming" "$runner_dir"
  /bin/rm -f "$archive"
fi
/bin/mkdir -p "$runner_dir/.tmp" "$runner_dir/.dotnet"
export TMPDIR="$runner_dir/.tmp"
export DOTNET_BUNDLE_EXTRACT_BASE_DIR="$runner_dir/.dotnet"

current_name=""
if [[ -f "$runner_dir/.runner" ]]; then
  current_name="$(/usr/bin/plutil -extract agentName raw -o - "$runner_dir/.runner" 2>/dev/null || true)"
fi
if [[ "$current_name" == "$runner_name" ]]; then
  /bin/rm -f "$token_file"
  cd "$runner_dir"
  exec ./run.sh "$@"
fi

if [[ ! -s "$token_file" ]]; then
  printf 'runner registration token is missing: %s\n' "$token_file" >&2
  exit 1
fi

backup_dir="$root/config-backup-$(/bin/date -u +%Y%m%dT%H%M%SZ)"
/bin/mkdir -m 700 "$backup_dir"
configuration_files=(.runner .runner_migrated .credentials .credentials_migrated .credentials_rsaparams)
for name in "${configuration_files[@]}"; do
  if [[ -f "$runner_dir/$name" ]]; then
    /bin/mv "$runner_dir/$name" "$backup_dir/$name"
  fi
done

restore_previous_configuration() {
  status=$?
  trap - ERR
  for name in "${configuration_files[@]}"; do
    if [[ -f "$backup_dir/$name" ]]; then
      /bin/mv -f "$backup_dir/$name" "$runner_dir/$name"
    fi
  done
  exit "$status"
}
trap restore_previous_configuration ERR

export ACTIONS_RUNNER_INPUT_TOKEN="$(/bin/cat "$token_file")"
export ACTIONS_RUNNER_INPUT_URL="$runner_url"
export ACTIONS_RUNNER_INPUT_NAME="$runner_name"
export ACTIONS_RUNNER_INPUT_LABELS="$runner_labels"
export ACTIONS_RUNNER_INPUT_WORK="_work"
/bin/rm -f "$token_file"

(
  cd "$runner_dir"
  ./config.sh --unattended --replace --disableupdate
)

trap - ERR
cd "$runner_dir"
exec ./run.sh "$@"
