//! Writing one subscription credential down: the shared write both the refresh
//! and the manual sign-in go through, the account it is written against, and
//! the reading that says a stored document is no longer usable.

use tracing::warn;

use super::super::super::standalone::{
    local_provider_credentials_enabled, put_local_subscription_credential,
};
use super::super::super::vault::{existing_item_account, existing_item_tags, put_credential};
use super::super::super::{redeem_subscription_credential, slug};
use super::super::tags::subscription_tags_for_write;
use crate::capability::Secret;

/// The harness a stored grant was borrowed from, for grants written before
/// borrowing was removed on 2026-09-20. Nothing writes this marker now; a
/// grant that still carries it is one Brama must not rotate, because the
/// machine it was taken from still holds the same pair.
pub(super) fn borrowed_from(credential: &Secret) -> Option<String> {
    let raw = credential.expose_utf8().ok()?;
    let blob: serde_json::Value = serde_json::from_str(raw).ok()?;
    blob.get(crate::subscription_dispatch::sign_in::manual::grant::BORROWED_FROM)?
        .as_str()
        .map(str::to_owned)
}

/// Store a rotated subscription credential, with the tags discovery requires.
///
/// This used to pass `None` for tags, which leaves whatever the item already
/// carried and gives a fresh item nothing at all. See
/// [`subscription_tags_for_write`] for what that cost.
///
/// Public because a grant does not only arrive by refresh: the manual sign-in
/// exchanges an authorization code the operator pasted and stores the result
/// the same way, so the two paths cannot disagree about tags or shape.
pub async fn put_subscription_credential(
    subscription_id: &str,
    provider: &str,
    credential: &[u8],
) -> Result<(), String> {
    put_subscription_credential_for_account(subscription_id, provider, credential, None).await
}

/// [`put_subscription_credential`] with the account the grant belongs to,
/// when the caller knows it: written as the item's `account_ref`, which is
/// the identity Weles signs the account in from once this grant dies, and as
/// its `brama:account:` tag, which is the only thing that says which of the
/// operator's accounts this member is without reading a name.
///
/// A caller that knows no account does not leave the item unattributed: the
/// account the item already records is read and written back, so the tag
/// appears on every member whose `account_ref` was recorded before the tag
/// existed, on the first credential write after this release, and nobody has
/// to stamp anything by hand.
pub async fn put_subscription_credential_for_account(
    subscription_id: &str,
    provider: &str,
    credential: &[u8],
    account: Option<&str>,
) -> Result<(), String> {
    let item_id = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    if local_provider_credentials_enabled() {
        return put_local_subscription_credential(&item_id, credential);
    }
    let recorded = match account {
        Some(account) => Some(account.to_owned()),
        None => existing_item_account(&item_id).await?,
    };
    let existing = existing_item_tags(&item_id).await?;
    let tags =
        subscription_tags_for_write(&existing, provider, subscription_id, recorded.as_deref())?;
    put_credential(&item_id, credential, Some(&tags), recorded.as_deref()).await
}

/// The account one subscription's item already names as `account_ref`, or
/// `None` when it names none - the state every imported member was in until
/// 2026-09-18, and the one Weles refuses to sign in from.
pub async fn subscription_account(
    subscription_id: &str,
    provider: &str,
) -> Result<Option<String>, String> {
    let item_id = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    if local_provider_credentials_enabled() {
        return Ok(None);
    }
    existing_item_account(&item_id).await
}

/// Why this stored document could not be presented to a provider, or nothing
/// when it can be.
///
/// The vault holding bytes for a subscription is not the same statement as the
/// subscription having a credential. A sign-in descriptor, an account record
/// and an empty envelope are all bytes; none of them is a grant, and none of
/// them becomes one by waiting, so the answer here decides whether the account
/// goes to Weles.
pub(super) fn unusable_document(
    subscription_id: &str,
    provider: &str,
    credential: &Secret,
) -> Option<String> {
    let item = format!("provider:{}:{}", slug(provider), slug(subscription_id));
    match credential.expose_utf8() {
        Ok(secret) => crate::providers::adapter::credential_key(&item, secret).err(),
        Err(error) => Some(format!(
            "Skarbiec item `{item}` holds bytes that are not valid UTF-8: {error}"
        )),
    }
}
