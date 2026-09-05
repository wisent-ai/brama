#!/bin/sh
# Restore the tags that bind a subscription credential to the agent allowed to
# spend it, on the items an operator names.
#
# The gateway enumerates accounts by tag: an item is a subscription only when it
# carries `brama:subscription` and `brama:agent:<agent>`. A rotation write
# performed by a binary from before the tag-preserving fix drops them, and the
# account then vanishes from the fleet while its credential stays perfectly
# valid -- which is why `/readyz` reported ready all day while kimi answered
# "no active 'kimi' credential for agent".
#
# This tool cannot discover its own subjects. It used to select them by id --
# every item named `provider:<provider>:<subscription-id>` -- and reconstruct
# the provider and the subscription id by splitting that id on colons. An item
# id in Skarbiec is a mutable, human-chosen name: a rename would silently move
# this repair onto the wrong item, or write `brama:provider:` and `brama:id:`
# values taken from whatever the name happened to spell, and nothing would
# raise. The tags being repaired are the only declaration of what an item is,
# and they are exactly what is missing here, so there is nothing left on the
# item to key on. A repair tool in that position must be given its subjects
# explicitly, and that is what this now requires: the operator states the item
# and the facts that were lost, and a rename then fails loudly on `retag`.
#
# What did survive is still authoritative. `brama:provider:<provider>` and
# `brama:id:<subscription-id>` are registered tag namespaces, and a write that
# lost `brama:agent:` may well have kept them; where a subject already declares
# one, that declaration wins and a contradicting argument is refused rather
# than applied.
#
# Usage:
#   restore-subscription-agent-tags-host.sh --agent <agent> \
#       --item <item-id> [--provider <provider>] [--subscription <id>] \
#       [--item ... ]...
#
# --provider and --subscription may be omitted for a subject that still carries
# the matching tag; they are required for one that does not.
#
# `retag` replaces tags only and never touches or re-encrypts the payload, so a
# live credential is not at risk. Requires a Skarbiec binary carrying `retag`.
# With an older one it refuses rather than reaching for `set-json`, because that
# path rewrites the secret.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
export PATH
GNUPGHOME="${GNUPGHOME:-$HOME/.gnupg}"
export GNUPGHOME
SKARBIEC="$HOME/.stado/bin/skarbiec"
SKARBIEC_VAULT_FILE="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
export SKARBIEC_VAULT_FILE
# The discriminator is the usage literal, not the bare command name: rustc packs
# string literals into one unterminated blob, so a binary that HAS the command
# shows `...setgetretagdelete...` on a single line and a whole-line match for
# `retag` reports absent on a build that carries it.
if ! strings -a "$SKARBIEC" 2>/dev/null | grep -q 'usage: retag <id> --tags'; then
  printf 'refusing: %s predates the tag-preserving retag command; ship the fixed build first\n' "$SKARBIEC" >&2
  exit 1
fi

usage() {
  printf 'usage: %s --agent <agent> --item <item-id> [--provider <p>] [--subscription <id>] [--item ...]...\n' "$0" >&2
  exit 2
}

AGENT=''
SUBJECTS=''
ITEM=''
PROVIDER=''
SUBSCRIPTION=''
TAB="$(printf '\t')"

flush_subject() {
  [ -n "$ITEM" ] || return 0
  SUBJECTS="$SUBJECTS$ITEM$TAB$PROVIDER$TAB$SUBSCRIPTION
"
  ITEM=''
  PROVIDER=''
  SUBSCRIPTION=''
}

while [ $# -gt 0 ]; do
  case "$1" in
    --agent)
      [ $# -ge 2 ] || usage
      AGENT="$2"
      shift 2
      ;;
    --item)
      [ $# -ge 2 ] || usage
      flush_subject
      ITEM="$2"
      shift 2
      ;;
    --provider)
      [ $# -ge 2 ] || usage
      [ -n "$ITEM" ] || usage
      PROVIDER="$2"
      shift 2
      ;;
    --subscription)
      [ $# -ge 2 ] || usage
      [ -n "$ITEM" ] || usage
      SUBSCRIPTION="$2"
      shift 2
      ;;
    *)
      usage
      ;;
  esac
done
flush_subject

[ -n "$AGENT" ] || usage
[ -n "$SUBJECTS" ] || usage

# The plan is built only from the subjects named on the command line. Each is
# looked up by its exact id -- an id this vault does not hold is an error, not a
# subject to skip -- and every tag that survived is preserved.
#
# The subjects travel as arguments, not on stdin: the reader below is fed the
# program itself through a heredoc, so anything piped in would be swallowed.
set -f
OLDIFS="$IFS"
IFS='
'
set -- $SUBJECTS
IFS="$OLDIFS"
set +f
PLAN="$(
  python3 - "$SKARBIEC_VAULT_FILE" "$AGENT" "$@" <<'PY'
import json, sys

vault_path, agent = sys.argv[1], sys.argv[2]
items = json.load(open(vault_path)).get("items", {})

PROVIDER_TAG = "brama:provider:"
ID_TAG = "brama:id:"


def declared(tags, prefix):
    found = [tag[len(prefix):] for tag in tags if tag.startswith(prefix)]
    if len(found) > 1:
        raise SystemExit(f"{prefix} is declared more than once; resolve it by hand")
    return found[0] if found else None


def settle(item_id, label, prefix, declared_value, given):
    if declared_value is not None:
        if given and given != declared_value:
            raise SystemExit(
                f"{item_id} already declares {prefix}{declared_value}; refusing the "
                f"contradicting --{label} {given}"
            )
        return declared_value
    if not given:
        raise SystemExit(
            f"{item_id} carries no {prefix} tag, so --{label} must state the {label} "
            "that was lost"
        )
    return given


plan = []
for line in sys.argv[3:]:
    if not line:
        continue
    item_id, given_provider, given_subscription = line.split("\t", 2)
    item = items.get(item_id)
    if item is None:
        raise SystemExit(f"no item {item_id} in {vault_path}")
    if item.get("state") != "active":
        raise SystemExit(f"{item_id} is {item.get('state')}, not active; refusing to retag it")
    tags = item.get("tags") or []
    if any(tag.startswith("brama:agent:") for tag in tags):
        print(f"# already routable, left alone: {item_id}", file=sys.stderr)
        continue
    provider = settle(item_id, "provider", PROVIDER_TAG, declared(tags, PROVIDER_TAG), given_provider)
    subscription = settle(item_id, "subscription", ID_TAG, declared(tags, ID_TAG), given_subscription)
    restored = [
        "brama:subscription",
        f"brama:agent:{agent}",
        f"{PROVIDER_TAG}{provider}",
        f"{ID_TAG}{subscription}",
    ]
    for tag in tags:      # keep whatever survived, e.g. brama:login:<item>
        if tag not in restored:
            restored.append(tag)
    plan.append(f"{item_id}\t{','.join(restored)}")

print("\n".join(plan))
PY
)"

if [ -z "$PLAN" ]; then
  printf 'nothing to restore: every named subject already declares an agent\n'
  exit 0
fi

printf '%s\n' "$PLAN" | while IFS="$TAB" read -r id tags; do
  [ -n "$id" ] || continue
  printf 'restoring %s\n    %s\n' "$id" "$tags"
  "$SKARBIEC" retag "$id" --tags "$tags" >/dev/null
done

printf '\n=== resulting tags\n'
set -f
OLDIFS="$IFS"
IFS='
'
set -- $PLAN
IFS="$OLDIFS"
set +f
python3 - "$SKARBIEC_VAULT_FILE" "$@" <<'PY'
import json, sys

items = json.load(open(sys.argv[1])).get("items", {})
for row in sys.argv[2:]:
    item_id = row.split("\t", 1)[0]
    item = items.get(item_id)
    if item is None:
        print(f"    {item_id} ABSENT")
        continue
    tags = item.get("tags") or []
    agent = "yes" if any(t.startswith("brama:agent:") for t in tags) else "NO"
    print(
        f"    {item_id} rev={item.get('revision')} state={item.get('state')} "
        f"routable={agent} tags={len(tags)}"
    )
PY
