//! Changing a remote gateway's pool membership: giving a member back,
//! forgetting one the pool only remembers, and putting a retired one back in
//! the rotation. Each states its reason, which the gateway keeps.

use serde_json::{json, Value};

use super::{answer, client, refusal};

/// Give one member back: the gateway retires it and forgets its credential.
///
/// This removes the members left over from the borrowing this product used
/// to allow: a grant taken from a machine that was also using it, which is
/// how a provider comes to revoke the session that machine was in.
pub(crate) async fn disown(
    gateway: &str,
    bearer: &str,
    subscription_id: &str,
    reason: &str,
) -> Result<String, String> {
    let subscription_id = subscription_id.trim();
    if subscription_id.is_empty() {
        return Err("name the subscription to disown".into());
    }
    if reason.trim().is_empty() {
        return Err("--reason must say why this member is given back".into());
    }
    let request = client()?
        .post(format!(
            "{}/v1/admin/subscription-pool/disown",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({"subscription_id": subscription_id, "reason": reason}));
    let response = stado_wait::request(request)
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let (status, body) = answer(response, gateway).await?;
    if !status.is_success() {
        return Err(format!(
            "the gateway refused to retire {subscription_id}: HTTP {status}: {}",
            refusal(&body)
        ));
    }
    Ok(format!(
        "{subscription_id} was retired: the gateway forgot its credential, and any machine that \
         signed that account in keeps its own session"
    ))
}

/// Drop one member the pool only remembers: the gateway writes the reason
/// to its journal and removes the usage history row, so the next pool
/// report no longer lists it.
pub(crate) async fn forget(
    gateway: &str,
    bearer: &str,
    subscription_id: &str,
    reason: &str,
) -> Result<String, String> {
    let subscription_id = subscription_id.trim();
    if subscription_id.is_empty() {
        return Err("name the subscription to forget".into());
    }
    if reason.trim().is_empty() {
        return Err("--reason must say why this member is forgotten".into());
    }
    let request = client()?
        .post(format!(
            "{}/v1/admin/subscription-pool/forget",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({"subscription_id": subscription_id, "reason": reason}));
    let response = stado_wait::request(request)
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let (status, body) = answer(response, gateway).await?;
    if !status.is_success() {
        return Err(format!(
            "the gateway refused to forget {subscription_id}: HTTP {status}: {}",
            refusal(&body)
        ));
    }
    match body.get("detail").and_then(Value::as_str) {
        Some(detail) => Ok(detail.to_owned()),
        None => Err(format!(
            "the gateway {gateway} accepted forgetting {subscription_id} but its answer carries \
             no detail: {body}"
        )),
    }
}

/// Put one retired member back in the rotation, because this deployment
/// uses that account after all.
///
/// A retirement was permanent: the gateway answered `no active credential
/// for agent` for a member it had given back, its own sign-in report said
/// `retired`, and no command in the product could take that back. This is
/// that command.
pub(crate) async fn reinstate(
    gateway: &str,
    bearer: &str,
    subscription_id: &str,
    reason: &str,
) -> Result<String, String> {
    let subscription_id = subscription_id.trim();
    if subscription_id.is_empty() {
        return Err("name the subscription to reinstate".into());
    }
    if reason.trim().is_empty() {
        return Err("--reason must say why this member is used again".into());
    }
    let request = client()?
        .post(format!(
            "{}/v1/admin/subscription-pool/reinstate",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({"subscription_id": subscription_id, "reason": reason}));
    let response = stado_wait::request(request)
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let (status, body) = answer(response, gateway).await?;
    if !status.is_success() {
        return Err(format!(
            "the gateway refused to reinstate {subscription_id}: HTTP {status}: {}",
            refusal(&body)
        ));
    }
    Ok(format!(
        "{subscription_id} is in the rotation again; it holds no grant of this gateway's own \
         until a sign-in obtains one"
    ))
}
