//! Register a discovered account without manufacturing or importing a credential.

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::super::router::{entitlements_router_bin, raw_listing, router_refusal};
use super::VaultListItem;

pub(in crate::gateway::broker) async fn register(
    provider: &str,
    id: &str,
    account: &str,
    metadata: &Value,
) -> Result<(), String> {
    let item = crate::gateway::broker::subscription_resource(provider, id);
    let listing = raw_listing(
        &entitlements_router_bin(),
        "inspect discovered account target",
    )
    .await?;
    let rows: Vec<VaultListItem> = serde_json::from_slice(&listing)
        .map_err(|error| format!("decode discovered account inventory: {error}"))?;
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
    let mut command = tokio::process::Command::new(entitlements_router_bin());
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
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| "account registration stdin is unavailable".to_owned())?;
    if let Err(error) = input.write_all(document.to_string().as_bytes()).await {
        let _ = child.kill().await;
        return Err(format!("write discovered account metadata: {error}"));
    }
    drop(input);
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
