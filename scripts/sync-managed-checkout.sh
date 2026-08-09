#!/usr/bin/env bash
set -euo pipefail

plist=/Library/LaunchDaemons/com.wisent.always-on.brama.plist
launcher=""
for index in $(seq 0 15); do
  argument=$(/usr/libexec/PlistBuddy -c "Print :ProgramArguments:${index}" "$plist" 2>/dev/null || true)
  [ -n "$argument" ] || continue
  printf 'argument[%s]=%s\n' "$index" "$argument"
  case "$argument" in
    */scripts/start-with-skarbiec.sh)
      launcher=$argument
      ;;
  esac
done

if [ -z "$launcher" ]; then
  echo "managed Brama unit does not run a source-checkout launcher" >&2
  exit 1
fi

repository=${launcher%/scripts/start-with-skarbiec.sh}
if [ ! -d "$repository/.git" ]; then
  echo "launcher repository is not a git checkout: $repository" >&2
  exit 1
fi

git -C "$repository" fetch origin main
git -C "$repository" merge --ff-only origin/main
printf 'repository=%s\nrevision=' "$repository"
git -C "$repository" rev-parse HEAD
