//! One vault item, as this gateway sees and replaces it.
//!
//! The row a `list` returns, the tags that row carries at this moment, and the
//! document a credential write puts in its place. It is one subject because
//! every write here is a read-modify-write of the same row: what the item
//! already carries decides what may be written back, and the two halves would
//! disagree if they lived apart.

use serde::Deserialize;
mod discovery;
pub(in crate::gateway::broker) use discovery::register as register_discovered_account;
pub(in crate::gateway::broker) use discovery::remove as remove_subscription_item;
pub(in crate::gateway::broker) use discovery::restore_mark as restore_subscription_mark;

use super::router::{entitlements_router_bin, raw_listing, router_output, router_refusal};

// Serialize metadata registration, grant publication and membership removal.
static MUTATIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One vault item row from the entitlements router's bare `list` command.
#[derive(Debug, Deserialize)]
pub(in crate::gateway::broker) struct VaultListItem {
    /// The coordinate the item lives at. Discovery reads tags, not ids -- but
    /// an item that has lost every tag still has this, and it is the only
    /// thing left to recognise a subscription account by.
    #[serde(default)]
    pub(in crate::gateway::broker) id: String,
    #[serde(default)]
    pub(in crate::gateway::broker) tags: Vec<String>,
    #[serde(default)]
    pub(in crate::gateway::broker) deleted: bool,
}

/// Every row the entitlements router lists, decoded. `purpose` names the
/// caller's question in a refusal, so a failed listing says what it was for.
pub(in crate::gateway::broker) async fn listed_items(
    purpose: &str,
) -> Result<Vec<VaultListItem>, String> {
    let stdout = raw_listing(&entitlements_router_bin()?, purpose).await?;
    serde_json::from_slice(&stdout)
        .map_err(|error| format!("decode the listing to {purpose}: {error}"))
}

/// The tags one live vault item carries right now; none when no live item
/// has that id. A trashed row's tags are not carried: `put_credential`
/// writes a fresh document over a trashed id, and its old tags would route
/// the new credential as the account that was removed.
pub(in crate::gateway::broker) async fn existing_item_tags(
    item_id: &str,
) -> Result<Vec<String>, String> {
    let items = listed_items("list vault tags for credential write").await?;
    Ok(
        match items
            .into_iter()
            .find(|item| item.id == item_id && !item.deleted)
        {
            Some(item) => item.tags,
            None => Vec::new(),
        },
    )
}

/// The principal one item already names as `context.account_ref`, or `None`
/// when the item is absent or names none. A sweep that hands an unchanged
/// grant over reads this before deciding there is nothing to write: a member
/// holding a grant the harness has not rotated and no account would otherwise
/// be answered "nothing to store" on every pass, and the account the harness
/// knows would never reach the item.
pub(in crate::gateway::broker) async fn existing_item_account(
    item_id: &str,
) -> Result<Option<String>, String> {
    let items = listed_items("inspect credential account").await?;
    if !items.iter().any(|item| item.id == item_id && !item.deleted) {
        return Ok(None);
    }
    let current = router_output("read credential account", |command| {
        command.arg("get").arg(item_id);
    })
    .await?;
    if !current.status.success() {
        return Err(router_refusal("read credential account", &current));
    }
    let document: serde_json::Value = serde_json::from_slice(&current.stdout)
        .map_err(|error| format!("decode credential account {item_id}: {error}"))?;
    Ok(document
        .pointer("/context/account_ref")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|account| !account.is_empty())
        .map(str::to_owned))
}

/// Replace one item's credential value, keeping what the item already
/// carries. `account` is the principal the credential belongs to, written
/// as `context.account_ref` when given: the field Weles resolves a sign-in
/// from, which an imported member does not carry on its own.
pub(in crate::gateway::broker) async fn put_credential(
    item_id: &str,
    secret: &[u8],
    tags: Option<&[String]>,
    account: Option<&str>,
) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let _mutation = MUTATIONS.lock().await;
    if let Some(id) = tags
        .into_iter()
        .flatten()
        .find_map(|tag| tag.strip_prefix("brama:id:"))
    {
        if crate::journal::membership_was_removed(id).map_err(|error| {
            format!("credential publication refused for subscription {id}: {error}")
        })? {
            return Err(format!("credential publication refused: subscription {id} was removed during reconciliation"));
        }
    }
    let items = listed_items("inspect credential target").await?;
    let mut document = if items.iter().any(|item| item.id == item_id && !item.deleted) {
        let current = router_output("read credential target metadata", |command| {
            command.arg("get").arg(item_id);
        })
        .await?;
        if !current.status.success() {
            return Err(router_refusal("read credential target metadata", &current));
        }
        serde_json::from_slice::<serde_json::Value>(&current.stdout)
            .map_err(|error| format!("decode credential target {item_id}: {error}"))?
    } else {
        serde_json::json!({"kind": "bundle", "schema": "skarbiec.item.v2",
            "context": {"source_kind": "donation"}, "fields": {}})
    };
    if let Some(account) = account.map(str::trim).filter(|account| !account.is_empty()) {
        let context = document
            .get_mut("context")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| {
                format!("credential target {item_id} has no canonical context object")
            })?;
        context.insert("account_ref".into(), serde_json::json!(account));
    }
    let fields = document
        .get_mut("fields")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| format!("credential target {item_id} has no canonical fields object"))?;
    let value = std::str::from_utf8(secret)
        .map_err(|error| format!("credential for {item_id} is not UTF-8: {error}"))?;
    fields.insert("value".into(), serde_json::json!(value));
    let kind = document
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("credential target {item_id} has no canonical kind"))?
        .to_owned();
    let document = document.to_string();
    let mut command = tokio::process::Command::new(entitlements_router_bin()?);
    command
        .kill_on_drop(true)
        .arg("set-json")
        .arg(item_id)
        .arg("--type")
        .arg(kind);
    if let Some(tags) = tags {
        command.arg("--tags").arg(tags.join(","));
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn credential write: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "credential write child stdin is unavailable".to_owned())?;
    if let Err(error) = stdin.write_all(document.as_bytes()).await {
        let stopped = match child.kill().await {
            Ok(()) => String::new(),
            Err(kill) => {
                format!("; the child could not be stopped and may store a partial document: {kill}")
            }
        };
        return Err(format!(
            "write credential document to child stdin: {error}{stopped}"
        ));
    }
    drop(stdin);
    let output = super::router::wait_within_footprint(child, "credential write").await?;
    if !output.status.success() {
        return Err(router_refusal("credential write", &output));
    }
    Ok(())
}
