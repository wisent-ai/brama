//! Register a discovered account without manufacturing or importing a credential.

use serde_json::{json, Value};

use super::super::router::{entitlements_router_bin, router_refusal};
use super::listed_items;

pub(in crate::gateway::broker) async fn register(
    provider: &str,
    id: &str,
    account: &str,
    metadata: &Value,
) -> Result<(), String> {
    let _mutation = super::MUTATIONS.lock().await;
    let item = crate::gateway::broker::subscription_resource(provider, id);
    let rows = listed_items("inspect discovered account target").await?;
    if rows.iter().any(|row| !row.deleted && row.id == item) {
        let existing = super::existing_item_account(&item).await?;
        return match existing {
            Some(existing) if existing.eq_ignore_ascii_case(account) => Ok(()),
            _ => Err(format!("subscription coordinate {item} already exists without this exact account identity; discovery does not overwrite it")),
        };
    }
    let document = json!({
        "schema": "skarbiec.item.v2", "kind": "bundle", "fields": {},
        "context": {
            "source_kind": "account_discovery", "provider": provider,
            "account_ref": account, "discovery": metadata,
        },
    });
    let tags =
        crate::gateway::broker::subscription_tags_for_write(&[], provider, id, Some(account))?;
    let mut command = tokio::process::Command::new(entitlements_router_bin()?);
    command
        .args([
            "set-json",
            &item,
            "--if-absent",
            "--type",
            "bundle",
            "--tags",
            &tags.join(","),
        ])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start discovered account registration: {error}"))?;
    super::super::router::feed_stdin(
        &mut child,
        document.to_string().as_bytes(),
        "register discovered account",
    )
    .await?;
    let output = stado_wait::child_output_async(
        child,
        format!("register discovered account {provider} {account}"),
    )
    .await
    .map_err(|error| format!("register discovered account: {error}"))?;
    if !output.status.success() {
        return Err(router_refusal(
            "register discovered account metadata",
            &output,
        ));
    }
    match super::existing_item_account(&item).await? {
        Some(stored) if stored.eq_ignore_ascii_case(account) => Ok(()),
        _ => Err(format!(
            "Skarbiec did not persist the discovered account identity on {item}"
        )),
    }
}

/// Remove only this member's tagged vault items, never its provider login.
pub(in crate::gateway::broker) async fn remove(
    provider: &str,
    id: &str,
    expected_account: Option<&str>,
) -> Result<(), String> {
    let _mutation = super::MUTATIONS.lock().await;
    let rows = listed_items("inspect obsolete subscription").await?;
    let member_tag = format!("brama:id:{id}");
    let provider_tag = format!("brama:provider:{provider}");
    for row in rows
        .iter()
        .filter(|row| !row.deleted && row.tags.contains(&member_tag))
    {
        if !row.tags.iter().any(|tag| tag == "brama:subscription") {
            return Err(format!(
                "refuse subscription removal {id}: vault item {} is not a subscription item",
                row.id
            ));
        }
        if !row.tags.contains(&provider_tag) {
            return Err(format!(
                "refuse subscription removal {id}: vault item {} has a different provider",
                row.id
            ));
        }
        let tagged = row
            .tags
            .iter()
            .find_map(|tag| tag.strip_prefix("brama:account:"));
        let recorded = super::existing_item_account(&row.id).await?;
        let current = recorded.as_deref().or(tagged);
        let unchanged = match (current, expected_account) {
            (None, None) => true,
            (Some(current), Some(expected)) => current.eq_ignore_ascii_case(expected),
            _ => false,
        };
        if !unchanged {
            return Err(format!(
                "refuse subscription removal {id}: account identity changed on {}",
                row.id
            ));
        }
        let output =
            super::super::router::router_output("remove obsolete subscription item", |command| {
                command.arg("delete").arg(&row.id);
            })
            .await?;
        if !output.status.success() {
            return Err(router_refusal("remove obsolete subscription item", &output));
        }
    }
    let remaining = listed_items("verify subscription removal").await?;
    if remaining
        .iter()
        .any(|row| !row.deleted && row.tags.contains(&member_tag))
    {
        return Err(format!(
            "subscription {id}: vault item remains after removal"
        ));
    }
    Ok(())
}

/// Restore the `brama:subscription` mark on a vault item that already
/// identifies an exact account (`brama:id:`, `brama:provider:`,
/// `brama:account:`) but lost the mark, so the pool, the renewal sweep and
/// Weles's login resolution see it again. The document is written back as it
/// is: no field is manufactured and no credential is imported. A tag the item
/// carries that disagrees with the identity asked for is refused by
/// [`crate::gateway::broker::subscription_tags_for_write`].
pub(in crate::gateway::broker) async fn restore_mark(
    provider: &str,
    id: &str,
    item_id: &str,
    account: &str,
) -> Result<(), String> {
    let _mutation = super::MUTATIONS.lock().await;
    let rows = listed_items("inspect recoverable account").await?;
    let row = rows
        .iter()
        .find(|row| !row.deleted && row.id == item_id)
        .ok_or_else(|| format!("subscription {id}: vault item {item_id} is absent"))?;
    let tags = crate::gateway::broker::subscription_tags_for_write(
        &row.tags,
        provider,
        id,
        Some(account),
    )?;
    let current = super::super::router::router_output("read recoverable account", |command| {
        command.arg("get").arg(item_id);
    })
    .await?;
    if !current.status.success() {
        return Err(router_refusal("read recoverable account", &current));
    }
    let document: Value = serde_json::from_slice(&current.stdout)
        .map_err(|error| format!("decode recoverable account {item_id}: {error}"))?;
    let kind = document
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("recoverable account {item_id} has no canonical kind"))?
        .to_owned();
    let mut command = tokio::process::Command::new(entitlements_router_bin()?);
    command
        .args([
            "set-json",
            item_id,
            "--type",
            &kind,
            "--tags",
            &tags.join(","),
        ])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start routing mark restoration: {error}"))?;
    super::super::router::feed_stdin(
        &mut child,
        document.to_string().as_bytes(),
        "restore routing mark",
    )
    .await?;
    let output = stado_wait::child_output_async(
        child,
        format!("restore routing mark of subscription {id} on {item_id}"),
    )
    .await
    .map_err(|error| format!("restore routing mark: {error}"))?;
    if !output.status.success() {
        return Err(router_refusal("restore routing mark", &output));
    }
    let rows = listed_items("verify routing mark").await?;
    if !rows.iter().any(|row| {
        !row.deleted && row.id == item_id && row.tags.iter().any(|tag| tag == "brama:subscription")
    }) {
        return Err(format!(
            "subscription {id}: Skarbiec did not persist the routing mark on {item_id}"
        ));
    }
    Ok(())
}
