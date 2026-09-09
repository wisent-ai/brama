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

# Which agents may use a subscription is read off the item, not out of a manifest.
#
# The item carries it: `brama:subscription` marks one and one `brama:agent:<id>`
# tag names each agent. A JSON list beside that was a second answer to the same
# question, and it disagreed -- the list on this host declared twenty-four
# subscriptions over twenty providers while the vault held six, and a paid Claude
# account was in the vault and missing from the list.
#
# These four namespaces are the whole vocabulary, and the listing already read
# above carries them, so this file opens the vault once through the router and
# never reads the vault file beside it.
SUBSCRIPTION_TAG = "brama:subscription"
PROVIDER_TAG = "brama:provider:"
SUBSCRIPTION_ID_TAG = "brama:id:"
AGENT_TAG = "brama:agent:"
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
# Subscription metadata comes from the item's declared tags, never from its id.
# A capability is intentionally not issued here. Skarbiec capabilities are
# short-lived and single-use, while the gateway already obtains one immediately
# before each credential redemption. Seeding every allowed resource delayed
# startup, rewrote the capability state once per resource, and produced ids that
# model discovery spent before the first request.
#
# `brama:subscription` marks a subscription, `brama:provider:<provider>` names
# its provider, `brama:id:<subscription-id>` names the subscription,
# `brama:login:<vault-item>` names the Weles account that can renew it, and each
# `brama:agent:<agent>` names an agent allowed to spend it. The policy still has
# to allow the exact provider resource; the catalog only exposes metadata and
# every use still requires a fresh capability from the authority.
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
    agent_ids = tag_values(tags, AGENT_TAG)
    login_item = tag_value(tags, LOGIN_TAG)
    missing = [
        f"{prefix}<value>"
        for prefix, value in (
            (PROVIDER_TAG, provider),
            (SUBSCRIPTION_ID_TAG, subscription_id),
            (AGENT_TAG, agent_ids),
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
        continue
    for agent_id in agent_ids:
        catalog.append({
            "id": subscription_id,
            "provider": provider,
            "agent_id": agent_id,
            "status": "active",
            "login_item": login_item,
        })

with open(catalog_path, "w", encoding="utf-8") as target:
    json.dump({"items": catalog}, target, separators=(",", ":"))
PY
printf '%s\n' "built runtime catalog; capabilities issue on demand" >/dev/stderr
export BRAMA_SUBSCRIPTION_CATALOG="$(cat "$catalog_file")"
# Boot-time ids are single-use and cannot be refreshed inside a running process.
# Clear inherited values so every credential use asks the authority at its final
# use boundary instead of trying a stale seed first.
unset BRAMA_PROVIDER_CAPABILITY_IDS BRAMA_REQUEST_SIGN_CAPABILITY_IDS
