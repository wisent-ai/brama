//! The tags a subscription credential write must leave on the vault item.
//!
//! This is the writer's half of the contract discovery reads, and it is its
//! own module because it is a refusal rather than a step: every path that
//! stores a subscription credential -- a rotation, a donation, a renewal --
//! has to meet it, and it answers only about tags, never about the value being
//! written.

use super::account::normalized_provider;

/// The tags a subscription credential write must store, given what the item
/// already carries.
///
/// Discovery finds an account by `brama:subscription` plus
/// `brama:agent:<agent>` (see `parse_live_subscriptions`), so an item missing
/// either is not a degraded account: it does not exist for any caller, while
/// its credential stays perfectly valid and every check that counts
/// credentials keeps answering green.
///
/// This is the writer's half of that contract, and it exists because the write
/// path had no such half. `put_subscription_credential` passed `None` for
/// tags, which means `skarbiec set-json` keeps whatever the item already had
/// and a fresh item is created with nothing -- so the rotation path could mint
/// a subscription that no agent could ever route to, and did.
///
/// Measured on charless-mac-mini on 2026-09-02: three of the four subscription
/// accounts in that vault -- `brama-sub-wisent-app-codex-secondary`,
/// `...-claude-primary`, `...-kimi-primary` -- carried `brama:provider:` and
/// `brama:id:` and neither `brama:subscription` nor any `brama:agent:`. Every
/// agent on that host could reach exactly one credential, so the single block
/// on it took the documentation gate of every repository down. One of the three
/// redeemed on the first probe after its tags were restored: a working paid
/// credential had been invisible the whole time. The same shape had already
/// cost this fleet a day through a missing `brama:agent:weles` tag.
///
/// The structural three are derived, never asked for: the provider and the
/// subscription id are what this write is for, and the mark follows from being
/// a subscription at all. The agent binding is the one thing that cannot be
/// derived -- it is an entitlement decision about who may spend a paid plan --
/// so a write that would leave an item with no agent tag is refused here
/// rather than completed with a guess. A refusal at write time is the only
/// place that requirement is met by whoever is doing the writing.
pub fn subscription_tags_for_write(
    existing: &[String],
    provider: &str,
    subscription_id: &str,
) -> Result<Vec<String>, String> {
    let mut tags: Vec<String> = existing.to_vec();
    if !tags.iter().any(|tag| tag == "brama:subscription") {
        tags.push("brama:subscription".to_owned());
    }
    for (prefix, wanted) in [
        ("brama:provider:", normalized_provider(provider)),
        ("brama:id:", subscription_id.to_owned()),
    ] {
        let declared = tags
            .iter()
            .filter_map(|tag| tag.strip_prefix(prefix))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let disagrees = declared.iter().any(|value| {
            if prefix == "brama:provider:" {
                normalized_provider(value) != wanted
            } else {
                *value != wanted.as_str()
            }
        });
        if disagrees {
            return Err(format!(
                "the vault item already carries {prefix}{}; refusing to write {prefix}{wanted} over it",
                declared.join(",")
            ));
        }
        if declared.is_empty() {
            tags.push(format!("{prefix}{wanted}"));
        }
    }
    if !tags.iter().any(|tag| {
        tag.strip_prefix("brama:agent:")
            .is_some_and(|a| !a.is_empty())
    }) {
        return Err(format!(
            "writing this credential would store subscription {subscription_id} for provider \
             {provider} with no 'brama:agent:<agent>' tag, so discovery could not see it and no \
             agent could route to it while its credential stayed valid. Which agents may spend a \
             paid plan is an entitlement decision this write cannot derive: tag the item with \
             `stado host retag-vault-item <host> provider:{provider}:{subscription_id} --tags …` \
             and repeat the write"
        ));
    }
    Ok(tags)
}
