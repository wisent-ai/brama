//! One vault item, as this gateway sees and replaces it.
//!
//! The row a `list` returns, the tags that row carries at this moment, and the
//! document a credential write puts in its place. It is one subject because
//! every write here is a read-modify-write of the same row: what the item
//! already carries decides what may be written back, and the two halves would
//! disagree if they lived apart.

use serde::Deserialize;

use super::router::{
    entitlements_router_bin, router_output, router_refusal, ENTITLEMENTS_ROUTER_TIMEOUT,
};

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

/// The tags one vault item carries right now, empty when it does not exist.
pub(in crate::gateway::broker) async fn existing_item_tags(
    item_id: &str,
) -> Result<Vec<String>, String> {
    let output = router_output("list vault tags for credential write", |command| {
        command.arg("list");
    })
    .await?;
    if !output.status.success() {
        return Err(router_refusal(
            "list vault tags for credential write",
            &output,
        ));
    }
    let items: Vec<VaultListItem> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("decode vault tags for credential write: {error}"))?;
    Ok(items
        .into_iter()
        .find(|item| item.id == item_id)
        .map(|item| item.tags)
        .unwrap_or_default())
}

pub(in crate::gateway::broker) async fn put_credential(
    item_id: &str,
    secret: &[u8],
    tags: Option<&[String]>,
) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let listing = router_output("inspect credential target", |command| {
        command.arg("list");
    })
    .await?;
    if !listing.status.success() {
        return Err(router_refusal("inspect credential target", &listing));
    }
    let items: Vec<VaultListItem> = serde_json::from_slice(&listing.stdout)
        .map_err(|error| format!("decode credential target inventory: {error}"))?;
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
    let mut command = tokio::process::Command::new(entitlements_router_bin());
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
    match tokio::time::timeout(
        ENTITLEMENTS_ROUTER_TIMEOUT,
        stdin.write_all(document.as_bytes()),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = child.kill().await;
            return Err(format!("write credential document to child stdin: {error}"));
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!(
                "write credential document timed out after {} seconds; the child was killed",
                ENTITLEMENTS_ROUTER_TIMEOUT.as_secs()
            ));
        }
    }
    drop(stdin);
    let output =
        match tokio::time::timeout(ENTITLEMENTS_ROUTER_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Err(format!("wait for credential write child: {error}")),
            Err(_) => {
                return Err(format!(
                    "credential write timed out after {} seconds; the child was killed",
                    ENTITLEMENTS_ROUTER_TIMEOUT.as_secs()
                ));
            }
        };
    if !output.status.success() {
        return Err(router_refusal("credential write", &output));
    }
    Ok(())
}
