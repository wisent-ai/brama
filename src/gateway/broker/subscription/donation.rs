//! A donated subscription: the row that records it and the credential it came
//! with.
//!
//! A donation is what a sign-in produces, and it is the only path that adds a
//! subscription to a running installation, so the two halves belong together:
//! the overlay file states that this host has the account at all, and the
//! credential write lands on the one coordinate reads for it resolve through.
//! Both must agree, and both refuse rather than guess -- an overlay row for a
//! credential nothing can read is inert, and a credential written under a
//! fresh id is unreadable by construction.

use std::path::PathBuf;

use serde_json::{json, Value};
use tracing::warn;

use super::super::standalone::{
    local_provider_credentials_enabled, put_local_subscription_credential,
};
use super::super::vault::{put_credential, router_output, router_refusal, VaultListItem};
use super::account::{parse_subscriptions, SubscriptionEntry};
use super::tags::subscription_tags_for_write;

const DONATED_SUBSCRIPTIONS_FILE_ENV: &str = "BRAMA_DONATED_SUBSCRIPTIONS_FILE";

/// Path of the donated-subscriptions overlay file.
pub fn donated_subscriptions_path() -> PathBuf {
    std::env::var(DONATED_SUBSCRIPTIONS_FILE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home)
                .join(".stado")
                .join("var")
                .join("brama")
                .join("donated-subscriptions.json")
        })
}

/// Overlay entries for one agent. A missing file is an empty overlay; every
/// other read or decode failure remains visible to the caller.
pub(super) fn donated_subscriptions(agent_id: &str) -> Result<Vec<SubscriptionEntry>, String> {
    let path = donated_subscriptions_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "read donated subscriptions file `{}`: {error}",
                path.to_string_lossy()
            ));
        }
    };
    parse_subscriptions(text.as_bytes(), agent_id).map_err(|error| {
        format!(
            "decode donated subscriptions file `{}`: {error}",
            path.to_string_lossy()
        )
    })
}

/// Read-modify-write the overlay file atomically (temp + rename, mode 0600).
fn update_donated_items(update: impl FnOnce(&mut Vec<Value>)) -> Result<(), String> {
    let path = donated_subscriptions_path();
    let mut items = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value.get("items").and_then(Value::as_array).cloned())
            .ok_or_else(|| "donated subscriptions file is corrupt".to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("read donated subscriptions file: {error}")),
    };
    update(&mut items);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create donated subscriptions dir: {error}"))?;
    }
    let payload = serde_json::to_string_pretty(&json!({"items": items}))
        .map_err(|error| format!("encode donated subscriptions: {error}"))?;
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|error| format!("write donated subscriptions file: {error}"))?;
        file.write_all(payload.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("write donated subscriptions file: {error}"))?;
    }
    std::fs::rename(&tmp, &path)
        .map_err(|error| format!("replace donated subscriptions file: {error}"))?;
    Ok(())
}

/// Record one donated subscription in the overlay file.
pub fn donated_add(
    agent_id: &str,
    id: &str,
    provider: &str,
    label: Option<&str>,
    login_item: Option<&str>,
) -> Result<(), String> {
    update_donated_items(|items| {
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(id));
        items.push(json!({
            "id": id,
            "provider": provider,
            "agent_id": agent_id,
            "status": "active",
            "label": label,
            "login_item": login_item,
        }));
    })
}

/// Drop one subscription id from the overlay file (no-op when absent).
pub fn donated_remove(id: &str) -> Result<(), String> {
    update_donated_items(|items| {
        items.retain(|item| item.get("id").and_then(Value::as_str) != Some(id));
    })
}

async fn donated_credential_tags(
    item_id: &str,
    agent_id: &str,
    provider: &str,
    subscription_id: &str,
    login_item: Option<&str>,
) -> Result<Vec<String>, DonationRefusal> {
    let output = router_output("list vault tags for donated credential", |command| {
        command.arg("list");
    })
    .await
    .map_err(DonationRefusal::Unwritable)?;
    if !output.status.success() {
        return Err(DonationRefusal::Unwritable(router_refusal(
            "list vault tags for donated credential",
            &output,
        )));
    }
    let items: Vec<VaultListItem> = serde_json::from_slice(&output.stdout).map_err(|error| {
        DonationRefusal::Unwritable(format!("decode vault tags for donated credential: {error}"))
    })?;
    let mut tags = items
        .into_iter()
        .find(|item| item.id == item_id)
        .map(|item| item.tags)
        .unwrap_or_default();

    let agent_tag = format!("brama:agent:{agent_id}");
    if !tags.contains(&agent_tag) {
        if tags.iter().any(|tag| {
            tag.strip_prefix("brama:agent:")
                .is_some_and(|agent| !agent.is_empty())
        }) {
            return Err(DonationRefusal::MappingConflict(format!(
                "{item_id} is not assigned to agent {agent_id}; refusing to replace its credential"
            )));
        }
        tags.push(agent_tag);
    }
    // Agent tags are additive entitlements, unlike the provider, subscription
    // and login identity. Renewing a shared credential must preserve every
    // existing consumer rather than reject the other authorized agents.
    let mut tags = subscription_tags_for_write(&tags, provider, subscription_id)
        .map_err(DonationRefusal::MappingConflict)?;
    if let Some(login_item) = login_item {
        let declared = tags
            .iter()
            .filter_map(|tag| tag.strip_prefix("brama:login:"))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if declared.iter().any(|value| *value != login_item) {
            return Err(DonationRefusal::MappingConflict(format!(
                "{item_id} is mapped to login {}; refusing credential minted by {login_item}",
                declared.join(",")
            )));
        }
        if declared.is_empty() {
            tags.push(format!("brama:login:{login_item}"));
        }
    }
    Ok(tags)
}

/// Why a donated credential was not banked.
///
/// The two are different repairs and different answers to the caller: a
/// document that carries no credential is the donor's to fix and left the vault
/// untouched, while a failed write is this installation's.
pub enum DonationRefusal {
    /// The document carries no credential the request path could present.
    Unusable(String),
    /// Existing subscription metadata names another account.
    MappingConflict(String),
    /// The vault write itself failed.
    Unwritable(String),
}

impl DonationRefusal {
    pub fn detail(&self) -> &str {
        match self {
            Self::Unusable(detail) | Self::MappingConflict(detail) | Self::Unwritable(detail) => {
                detail
            }
        }
    }
}

/// Store one donated OAuth credential blob through the local entitlements
/// router; plaintext crosses only the child process stdin pipe.
///
/// The item is the one the operator's routes table already names for this
/// subscription. Minting a fresh id per donation produced a credential nothing
/// could read: reads resolve through that table, and a new id has no entry in
/// it, so the donation was inert by construction.
///
/// A donation is refused unless the document reduces to a bearer, because this
/// write lands on the one coordinate a provider's `-primary` subscription is
/// read from and there is no second copy. On 2026-08-19 the vault item
/// `provider:codex:brama-sub-wisent-app-codex-primary` -- an account with 11,123
/// recorded requests -- held a browser context options document
/// (`deviceScaleFactor`, `extraHTTPHeaders`, `recordHar`, `recordVideo`,
/// `viewport`) at revision 318, so every call routed to it was refused by the
/// provider-authentication check while the ledger read `active`: the sign-in
/// this records had marked it so. The only length bound the boundary applied was
/// 1..8000 characters, which a re-authentication trajectory's own configuration
/// object satisfies. The predicate is the request path's own reduction, so
/// nothing a request could have presented is refused here.
pub async fn put_donated_credential(
    agent_id: &str,
    provider: &str,
    subscription_id: &str,
    api_key: &str,
    login_item: Option<&str>,
) -> Result<(), DonationRefusal> {
    let item_id = format!("provider:{provider}:{subscription_id}");
    // The bearer is derived only to prove one can be, and dropped unread.
    if let Err(detail) = crate::providers::adapter::credential_key(&item_id, api_key) {
        warn!(
            event = "donated_credential_unusable",
            provider,
            subscription = subscription_id,
            %detail,
            "a donated document carries no credential; the stored one is left as it is"
        );
        return Err(DonationRefusal::Unusable(detail));
    }
    if local_provider_credentials_enabled() {
        put_local_subscription_credential(&item_id, api_key.as_bytes())
            .map_err(DonationRefusal::Unwritable)?;
    } else {
        let tags =
            donated_credential_tags(&item_id, agent_id, provider, subscription_id, login_item)
                .await?;
        put_credential(&item_id, api_key.as_bytes(), Some(&tags))
            .await
            .map_err(DonationRefusal::Unwritable)?;
    }
    crate::subscription_dispatch::usage::record_credential_signed_in(subscription_id, provider);
    Ok(())
}
