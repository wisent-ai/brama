subscriptions_file="$runtime_dir/subscriptions.json"
catalog_file="$runtime_dir/subscription-catalog.json"
# Releases before 0.2.52 persisted boot-time capability ids here. They are
# single-use authority records, not durable service state.
rm -f "$runtime_dir/provider-capabilities.json" "$runtime_dir/request-sign-capabilities.json"
printf '%s\n' "reading subscription catalog" >/dev/stderr
"$ENTITLEMENTS_ROUTER_BIN" list >"$subscriptions_file"
printf '%s\n' "read subscription catalog; building runtime catalog" >/dev/stderr
"$PYTHON_BIN" - \
  "$subscriptions_file" \
  "$config_dir/policy.json" \
  "$catalog_file" <<'PY'
import json
import sys

(
    _program,
    available_path,
    policy_path,
    catalog_path,
) = sys.argv
with open(available_path, encoding="utf-8") as source:
    available_items = json.load(source)
with open(policy_path, encoding="utf-8") as source:
    policy = json.load(source)

# Which subscriptions exist is read off the items, not out of a manifest.
#
# The item carries it: `brama:subscription` marks one, `brama:provider:` and
# `brama:id:` name it. A JSON list beside that was a second answer to the same
# question, and it disagreed -- the list on this host declared twenty-four
# subscriptions over twenty providers while the vault held six, and a paid Claude
# account was in the vault and missing from the list.
#
# Until 2026-09-16 the catalog also carried one row per `brama:agent:<id>` tag
# and the gateway served an agent only the rows tagged for it; two paid Claude
# accounts with no agent tag served nobody. A subscription in the vault is in
# the rotation for every caller, so a row names no agent.
SUBSCRIPTION_TAG = "brama:subscription"
PROVIDER_TAG = "brama:provider:"
SUBSCRIPTION_ID_TAG = "brama:id:"
LOGIN_TAG = "brama:login:"


def tag_values(tags, prefix):
    return [tag[len(prefix):] for tag in tags if tag.startswith(prefix) and tag != prefix]


def tag_value(tags, prefix):
    declared = tag_values(tags, prefix)
    return declared[0] if declared else None

rules = policy.get("roles", {}).get("brama-runtime", [])
allowed = {
    (rule.get("purpose"), rule.get("resource"))
    for rule in rules
    if isinstance(rule, dict)
}
normalize = lambda value: value.strip().lower().replace("_", "-")


catalog = []
# Subscriptions the vault holds and the runtime policy does not name. Counted
# so the boot log ends with one line an operator can act on rather than a
# scatter of skips.
unnamed = []
# Subscription metadata comes from the item's declared tags, never from its id.
# A capability is intentionally not issued here. Skarbiec capabilities are
# short-lived and single-use, while the gateway already obtains one immediately
# before each credential redemption. Seeding every allowed resource delayed
# startup, rewrote the capability state once per resource, and produced ids that
# model discovery spent before the first request.
#
# `brama:subscription` marks a subscription, `brama:provider:<provider>` names
# its provider, `brama:id:<subscription-id>` names the subscription and
# `brama:login:<vault-item>` names the Weles account that can renew it. The
# policy still has to allow the exact provider resource; the catalog only
# exposes metadata and every use still requires a fresh capability from the
# authority.
for item in available_items:
    if not isinstance(item, dict) or item.get("deleted", False):
        continue
    item_name = item.get("id")
    if not isinstance(item_name, str):
        continue
    tags = item.get("tags") or []
    if SUBSCRIPTION_TAG not in tags:
        continue
    provider = tag_value(tags, PROVIDER_TAG)
    subscription_id = tag_value(tags, SUBSCRIPTION_ID_TAG)
    login_item = tag_value(tags, LOGIN_TAG)
    missing = [
        f"{prefix}<value>"
        for prefix, value in (
            (PROVIDER_TAG, provider),
            (SUBSCRIPTION_ID_TAG, subscription_id),
        )
        if not value
    ]
    if missing:
        sys.stderr.write(
            f"skipping {item_name}: carries {SUBSCRIPTION_TAG} but no "
            f"{' and no '.join(missing)} tag, so nothing declares what it serves; "
            "tag it and it is served again\n"
        )
        continue
    provider = normalize(provider)
    resource = f"provider:{provider}:{subscription_id}"
    if ("brama.provider.authenticate", resource) not in allowed:
        # Until 2026-09-20 this was a bare `continue`: a paid account the vault
        # held, tagged correctly, simply did not exist for the gateway, and the
        # only trace was a refresh answering `no capability route maps this
        # resource to a vault item and field` hours later. The runtime policy is
        # generated from the vault's own tags when the release is installed, so
        # an item added after that install is absent from it until the next one.
        unnamed.append(resource)
        sys.stderr.write(
            f"skipping {item_name}: the runtime policy has no "
            f"brama.provider.authenticate rule for {resource}, so no capability route maps "
            "this resource to a vault item and field; the policy is generated from the "
            "vault's tags when the release is installed, so an item added since then is "
            "served again after the next install of this release on this host\n"
        )
        continue
    catalog.append({
        "id": subscription_id,
        "provider": provider,
        "status": "active",
        "login_item": login_item,
    })

with open(catalog_path, "w", encoding="utf-8") as target:
    json.dump({"items": catalog}, target, separators=(",", ":"))
if unnamed:
    sys.stderr.write(
        f"{len(unnamed)} subscription(s) the runtime policy does not name are not served: "
        f"{', '.join(sorted(unnamed))}; install this release again on this host to regenerate "
        "the policy from the vault's current tags\n"
    )
sys.stderr.write(f"{len(catalog)} subscription(s) served by this gateway\n")
PY
printf '%s\n' "built runtime catalog; capabilities issue on demand" >/dev/stderr
export BRAMA_SUBSCRIPTION_CATALOG="$(cat "$catalog_file")"
# Boot-time ids are single-use and cannot be refreshed inside a running process.
# Clear inherited values so every credential use asks the authority at its final
# use boundary instead of trying a stale seed first.
unset BRAMA_PROVIDER_CAPABILITY_IDS BRAMA_REQUEST_SIGN_CAPABILITY_IDS
