//! The tags a subscription credential write must leave on the vault item,
//! and how the account among them comes to be recorded.
//!
//! This is the writer's half of the contract discovery reads, and it is its
//! own module because it is a refusal rather than a step: every path that
//! stores a subscription credential -- a rotation, a donation, a renewal --
//! has to meet it.

use super::super::slug;

use super::super::vault::{existing_item_account, existing_item_tags, put_credential};
use super::account::normalized_provider;
use crate::capability::Secret;
use crate::gateway::oauth_refresh;

/// The tags a subscription credential write must store, given what the item
/// already carries.
///
/// Discovery finds an account by `brama:subscription` (see
/// `parse_live_subscriptions`), so an item missing the mark is not a degraded
/// account: it does not exist for any caller, while its credential stays
/// perfectly valid and every check that counts credentials keeps answering
/// green.
///
/// This is the writer's half of that contract, and it exists because the write
/// path had no such half. `put_subscription_credential` passed `None` for
/// tags, which means `skarbiec set-json` keeps whatever the item already had
/// and a fresh item is created with nothing -- so the rotation path could mint
/// a subscription that no agent could ever route to, and did.
///
/// An item that lost the mark this way keeps a perfectly valid credential
/// while no agent can reach it, and a pool narrowed to one reachable
/// credential is one block away from serving nothing at all. Restoring the
/// tags makes such a credential redeemable again on the first probe, which
/// is the shape of the defect: nothing was wrong with the grant.
///
/// All four tags are derived, never asked for: the provider and the
/// subscription id are what this write is for, the mark follows from being a
/// subscription at all, and the account is the principal whose grant is
/// being stored. The write derives all of them; an entitlement decision is
/// not among them, because every subscription serves every caller.
///
/// `brama:account:` is the one fact that says which provider account a
/// member is. Before it existed, every reader had to fall back on a name --
/// the member's own id, its label, or the login row it signs in through --
/// and a name is not an account: one Google login row can back both a
/// Claude Code and a Codex subscription of one person, so counting login
/// rows reports two accounts as one and a login of one provider as an
/// account of another. The tag is written from what the caller already
/// resolved, and a write that disagrees with what the item carries is
/// refused rather than silently repointed.
pub fn subscription_tags_for_write(
    existing: &[String],
    provider: &str,
    subscription_id: &str,
    account: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut tags: Vec<String> = existing.to_vec();
    if !tags.iter().any(|tag| tag == "brama:subscription") {
        tags.push("brama:subscription".to_owned());
    }
    let account = account.map(str::trim).filter(|account| !account.is_empty());
    let declarations = [
        Some(("brama:provider:", normalized_provider(provider))),
        Some(("brama:id:", subscription_id.to_owned())),
        account.map(|account| ("brama:account:", account.to_owned())),
    ];
    for (prefix, wanted) in declarations.into_iter().flatten() {
        let declared = tags
            .iter()
            .filter_map(|tag| tag.strip_prefix(prefix))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        // An address is one account however it is capitalised, and a
        // provider name is one provider whichever separator it is spelled
        // with; anything else is compared exactly.
        let disagrees = declared.iter().any(|value| match prefix {
            "brama:provider:" => normalized_provider(value) != wanted,
            "brama:account:" => !value.eq_ignore_ascii_case(&wanted),
            _ => *value != wanted.as_str(),
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
    Ok(tags)
}

/// Record the account a stored grant states, when the item states none.
///
/// Called by the sweep, which already holds the decrypted grant of every
/// member on every pass, so no extra read of credential material happens for
/// it. The account is the provider's own claim about the credential -- an
/// address Claude Code records beside the grant and Codex signs into the
/// identity token next to the access token -- and it is written where every
/// later reader looks: `context.account_ref`, which Weles resolves a sign-in
/// from, and `brama:account:`, which the pool counts accounts by.
///
/// Why the sweep and not a one-off repair: a member imported before this
/// existed carries no account, so the pool can attribute only the members
/// this gateway has itself signed in and reports the rest as unattributed.
/// Recording it where the grant is already open means the next member
/// imported the same way is attributed without anybody stamping anything.
///
/// Best effort by contract: an account that cannot be read or cannot be
/// written leaves the member unattributed, which the pool reports, and never
/// fails the sweep that was refreshing a credential.
pub(in crate::gateway::broker) async fn record_stated_account(
    subscription_id: &str,
    provider: &str,
    credential: &Secret,
) -> Result<Option<String>, String> {
    let item_id = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    if let Some(recorded) = existing_item_account(&item_id).await? {
        return Ok(Some(recorded));
    }
    let Some(account) = oauth_refresh::stated_account(credential, provider) else {
        return Ok(None);
    };
    let raw = credential
        .expose_utf8()
        .map_err(|error| format!("credential for {item_id} is not UTF-8: {error}"))?;
    let existing = existing_item_tags(&item_id).await?;
    let tags = subscription_tags_for_write(&existing, provider, subscription_id, Some(&account))?;
    match put_credential(&item_id, raw.as_bytes(), Some(&tags), Some(&account)).await {
        Ok(()) => Ok(Some(account)),
        // A vault older than the `brama:account:` namespace refuses the tag
        // by name. The account itself is not the tag: it is recorded as the
        // item's own `account_ref`, which every reader of this can resolve,
        // and the tag is the cheap index that vault cannot hold yet. So the
        // account is still recorded, and the deployment is not left
        // unattributed until its vault is delivered.
        Err(refused) if refused.contains("namespace that is not registered") => {
            let without_account =
                subscription_tags_for_write(&existing, provider, subscription_id, None)?;
            put_credential(
                &item_id,
                raw.as_bytes(),
                Some(&without_account),
                Some(&account),
            )
            .await?;
            Ok(Some(account))
        }
        Err(refused) => Err(refused),
    }
}

/// [`record_stated_account`] for a caller that does not hold the credential:
/// the member's grant is redeemed, the account it states is recorded, and
/// what the item now records is returned.
///
/// This is what `brama subscription attribute` runs and what the sweep does
/// on its own pass, so an operator asking which accounts this deployment
/// holds and a gateway serving traffic cannot disagree about it. Nothing is
/// rotated: a grant is opened, read for the address its issuer put in it, and
/// written back unchanged beside the account it names.
pub async fn record_subscription_account(
    subscription_id: &str,
    provider: &str,
) -> Result<Option<String>, String> {
    let item_id = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    if let Some(recorded) = existing_item_account(&item_id).await? {
        return Ok(Some(recorded));
    }
    let credential = super::super::redeem_subscription_credential(subscription_id, provider)
        .await
        .map_err(|refused| {
            refused
                .detail
                .unwrap_or_else(|| "the credential could not be redeemed".to_owned())
        })?;
    record_stated_account(subscription_id, provider, &credential).await
}
