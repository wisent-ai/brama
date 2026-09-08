//! One subscription account, and how each source's row becomes one.
//!
//! Three sources describe the same thing -- the vault listing's tags, the
//! trusted catalog in the environment, and the donated overlay's JSON -- and
//! every screen, the pool, the CLI and the refresh all read the row this
//! produces. They are together because they must agree: a provider spelled
//! `claude_code` in one listing and `claude-code` in another is two accounts
//! to everything downstream, and an incomplete row that still identifies an
//! exact subscription must be recognisable as such rather than routable.

use serde::Deserialize;

use super::super::vault::VaultListItem;

const SUBSCRIPTION_CATALOG_ENV: &str = "BRAMA_SUBSCRIPTION_CATALOG";

#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionEntry {
    pub id: String,
    pub provider: String,
    pub status: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub login_item: Option<String>,
}

/// One Brama credential whose metadata is not complete enough to route.
///
/// Deliberately not a [`SubscriptionEntry`]: these are precisely the accounts
/// the per-agent listing cannot produce, and giving them the routable type
/// would invite a caller to route to one. Provider and id come only from tags;
/// item names stay opaque.
#[derive(Debug, Clone)]
pub struct UnroutableAccount {
    /// The subscription id from `brama:id:` tag, or None when that tag is absent.
    pub id: Option<String>,
    /// The provider from `brama:provider:` tag, or None when that tag is absent.
    pub provider: Option<String>,
    /// The Weles vault row from `brama:login:`, when an earlier write kept it.
    pub login_item: Option<String>,
    /// The vault item id it lives in, so diagnostics have an address.
    pub item: String,
}

#[derive(Debug, Deserialize)]
struct BrokerItems {
    #[serde(default)]
    items: Vec<BrokerSubscriptionEntry>,
}

#[derive(Debug, Deserialize)]
struct BrokerSubscriptionEntry {
    id: Option<String>,
    provider: Option<String>,
    agent_id: Option<String>,
    status: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    login_item: Option<String>,
}

pub(in crate::gateway::broker) fn configured_subscription_ids() -> std::collections::HashSet<String>
{
    let Some(catalog) = std::env::var(SUBSCRIPTION_CATALOG_ENV)
        .ok()
        .and_then(|encoded| serde_json::from_str::<BrokerItems>(&encoded).ok())
    else {
        return std::collections::HashSet::new();
    };
    catalog
        .items
        .into_iter()
        .filter_map(|entry| entry.id)
        .collect()
}

fn complete_field(value: Option<String>) -> Option<String> {
    value.filter(|field| !field.is_empty() && field.trim() == field)
}

fn required_broker_field(
    value: Option<String>,
    field: &str,
    index: usize,
) -> Result<String, String> {
    complete_field(value)
        .ok_or_else(|| format!("subscription catalog row {index} is missing valid field `{field}`"))
}

pub(super) fn parse_subscriptions(
    output: &[u8],
    agent_id: &str,
) -> Result<Vec<SubscriptionEntry>, String> {
    let response: BrokerItems = serde_json::from_slice(output)
        .map_err(|error| format!("subscription catalog contains malformed JSON: {error}"))?;
    let mut entries = Vec::new();
    for (index, entry) in response.items.into_iter().enumerate() {
        let entry_agent_id = required_broker_field(entry.agent_id, "agent_id", index)?;
        if entry_agent_id != agent_id {
            continue;
        }
        entries.push(SubscriptionEntry {
            id: required_broker_field(entry.id, "id", index)?,
            provider: required_broker_field(entry.provider, "provider", index)?,
            status: required_broker_field(entry.status, "status", index)?,
            label: complete_field(entry.label),
            login_item: complete_field(entry.login_item),
        });
    }
    Ok(entries)
}

pub(super) fn configured_subscriptions(
    agent_id: &str,
) -> Option<Result<Vec<SubscriptionEntry>, String>> {
    let encoded = std::env::var(SUBSCRIPTION_CATALOG_ENV).ok()?;
    Some(parse_subscriptions(encoded.as_bytes(), agent_id))
}

fn subscription_tag_value<'a>(tags: &'a [String], prefix: &str) -> Option<&'a str> {
    tags.iter().find_map(|tag| {
        tag.strip_prefix(prefix)
            .and_then(|value| (!value.is_empty()).then_some(value))
    })
}

/// Map the router's full vault listing to one agent's subscriptions. An item
/// is a subscription when it carries the `brama:subscription` tag plus
/// `brama:agent:<agent>`; `brama:provider:` and `brama:id:` tags carry the
/// provider and subscription id, so item ids stay opaque and renames are safe.
/// Non-deleted resources become active entries.
fn live_subscription_entry(item: &VaultListItem) -> Result<SubscriptionEntry, String> {
    let coordinate = if item.id.trim().is_empty() {
        "<unnamed vault item>"
    } else {
        item.id.as_str()
    };
    let id = subscription_tag_value(&item.tags, "brama:id:").ok_or_else(|| {
        format!("subscription vault item `{coordinate}` is missing tag `brama:id:<id>`")
    })?;
    let provider = subscription_tag_value(&item.tags, "brama:provider:").ok_or_else(|| {
        format!("subscription vault item `{coordinate}` is missing tag `brama:provider:<provider>`")
    })?;
    Ok(SubscriptionEntry {
        id: id.to_owned(),
        provider: normalized_provider(provider),
        status: "active".to_owned(),
        label: None,
        login_item: subscription_tag_value(&item.tags, "brama:login:").map(str::to_owned),
    })
}

pub(super) fn parse_live_subscriptions(
    output: &[u8],
    agent_id: &str,
) -> Result<Vec<SubscriptionEntry>, String> {
    let agent_tag = format!("brama:agent:{agent_id}");
    let items: Vec<VaultListItem> = serde_json::from_slice(output)
        .map_err(|error| format!("subscription listing returned malformed JSON: {error}"))?;
    let mut entries = Vec::new();
    for item in items.into_iter().filter(|item| {
        !item.deleted
            && item.tags.iter().any(|tag| tag == "brama:subscription")
            && item.tags.iter().any(|tag| tag == &agent_tag)
    }) {
        entries.push(live_subscription_entry(&item)?);
    }
    Ok(entries)
}

/// The same mapping without the owning-agent filter, for the console listing.
pub(super) fn parse_live_subscriptions_any_agent(
    output: &[u8],
) -> Result<Vec<SubscriptionEntry>, String> {
    let items: Vec<VaultListItem> = serde_json::from_slice(output)
        .map_err(|error| format!("subscription listing returned malformed JSON: {error}"))?;
    let mut entries = Vec::new();
    for item in items
        .into_iter()
        .filter(|item| !item.deleted && item.tags.iter().any(|tag| tag == "brama:subscription"))
    {
        entries.push(live_subscription_entry(&item)?);
    }
    Ok(entries)
}

/// Normalize a provider name the way both live parsers above do, so one
/// account cannot be `claude_code` on one listing and `claude-code` on another.
pub(super) fn normalized_provider(value: &str) -> String {
    value.trim().to_lowercase().replace('_', "-")
}

/// Map the router's listing to Brama credentials that cannot be routed.
///
/// A complete subscription needs the marker and at least one agent tag. An
/// older credential write could preserve `brama:id:` and `brama:provider:` but
/// drop either routing tag; those items are included so automatic renewal can
/// restore their metadata. Items with no Brama id remain unrelated vault data.
pub(super) fn parse_unroutable_accounts(output: &[u8]) -> Result<Vec<UnroutableAccount>, String> {
    let items: Vec<VaultListItem> = serde_json::from_slice(output)
        .map_err(|error| format!("vault account listing returned malformed JSON: {error}"))?;
    let mut accounts: Vec<UnroutableAccount> = items
        .into_iter()
        .filter(|item| !item.deleted)
        .filter(|item| subscription_tag_value(&item.tags, "brama:id:").is_some())
        .filter(|item| subscription_tag_value(&item.tags, "brama:provider:").is_some())
        .filter(|item| {
            !item.tags.iter().any(|tag| tag == "brama:subscription")
                || subscription_tag_value(&item.tags, "brama:agent:").is_none()
        })
        .map(|item| {
            let id = subscription_tag_value(&item.tags, "brama:id:").map(str::to_owned);
            let provider =
                subscription_tag_value(&item.tags, "brama:provider:").map(normalized_provider);
            let login_item = subscription_tag_value(&item.tags, "brama:login:").map(str::to_owned);
            UnroutableAccount {
                id,
                provider,
                login_item,
                item: item.id,
            }
        })
        .collect();
    accounts.sort_by(|left, right| left.item.cmp(&right.item));
    Ok(accounts)
}
