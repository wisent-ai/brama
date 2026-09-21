//! Reading and administering the pool of a gateway that runs somewhere else.
//!
//! The gateway an operator asks about is usually not this process: it runs on
//! the fleet's host and this is a workstation. Reading its pool, and giving
//! back a member it should not hold, are the two things that may cross
//! machines, because neither hands a credential to anybody. Handing a grant
//! over is not a capability of this product: a provider issues one OAuth pair
//! per sign-in and revokes it when a second holder refreshes, so a gateway
//! signs itself in rather than taking a pair from a machine already using it.

use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::unattended;

/// A gateway answers a pool read inside a minute, or it is not answering.
const POOL_READ_DEADLINE_SECONDS: u64 = 60;
const HTTP_SUCCESS: std::ops::Range<u16> = 200..300;

/// Which gateway a command speaks to, and what it authenticates with.
#[derive(Clone, Default)]
pub(crate) struct Destination {
    pub gateway: Option<String>,
    pub gateway_consumer: Option<String>,
    pub bearer_item: Option<String>,
}

impl Destination {
    /// The origin and console bearer this call uses, or nothing when the
    /// caller means this process's own pool.
    pub(crate) async fn resolve(
        &self,
        stdin_bearer: &Zeroizing<String>,
    ) -> Result<(Option<String>, Zeroizing<String>), String> {
        let gateway = match (&self.gateway, &self.gateway_consumer) {
            (Some(_), Some(_)) => {
                return Err("name --gateway or --gateway-consumer, not both".into())
            }
            (Some(gateway), None) => Some(gateway.clone()),
            (None, Some(consumer)) => Some(unattended::gateway_for_consumer(consumer).await?),
            (None, None) => None,
        };
        if gateway.is_none() {
            if self.bearer_item.is_some() {
                return Err("--bearer-item is for a gateway; name one".into());
            }
            return Ok((None, Zeroizing::new(String::new())));
        }
        let bearer = match &self.bearer_item {
            Some(coordinate) => unattended::bearer_from_vault(coordinate).await?,
            None => stdin_bearer.clone(),
        };
        if bearer.trim().is_empty() {
            return Err(
                "a gateway needs the console's bearer: --bearer-item, or the token on stdin".into(),
            );
        }
        Ok((gateway, bearer))
    }

    /// The same, reading stdin itself when no vault coordinate was named.
    pub(crate) async fn resolve_reading_stdin(
        &self,
    ) -> Result<(Option<String>, Zeroizing<String>), String> {
        let mut stdin_bearer = Zeroizing::new(String::new());
        if self.gateway.is_some() && self.bearer_item.is_none() {
            use std::io::Read as _;
            std::io::stdin()
                .read_to_string(&mut stdin_bearer)
                .map_err(|error| format!("reading the console bearer from stdin: {error}"))?;
        }
        self.resolve(&stdin_bearer).await
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(POOL_READ_DEADLINE_SECONDS))
        .build()
        .map_err(|error| format!("gateway client: {error}"))
}

/// One gateway's pool, as it reports it.
pub(crate) async fn pool_report(gateway: &str, bearer: &str) -> Result<Value, String> {
    let response = client()?
        .get(format!(
            "{}/v1/subscription-pool",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .send()
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("the gateway's pool report is not JSON: {error}"))?;
    if !HTTP_SUCCESS.contains(&status) {
        return Err(format!(
            "the gateway refused the pool report with HTTP {status}: {}",
            refusal(&body)
        ));
    }
    Ok(body)
}

/// Give one member back: the gateway retires it and forgets its credential.
///
/// This removes the members left over from the borrowing this product used to
/// allow — the three claude-code accounts taken from a workstation on
/// 2026-09-20, each of them a pair that workstation was also using, which is
/// why the operator was signed out of Claude by hand that day.
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
    let response = client()?
        .post(format!(
            "{}/v1/admin/subscription-pool/disown",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({"subscription_id": subscription_id, "reason": reason}))
        .send()
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !HTTP_SUCCESS.contains(&status) {
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

/// Put one retired member back in the rotation, because the operator says
/// that account is one this deployment uses.
///
/// A retirement was permanent, and on 2026-09-21 all five of the operator's
/// accounts were retired: the gateway answered `no active credential for
/// agent` for each of them, its own sign-in report said `retired`, and no
/// command in the product could say they were his. This is that command.
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
    let response = client()?
        .post(format!(
            "{}/v1/admin/subscription-pool/reinstate",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({"subscription_id": subscription_id, "reason": reason}))
        .send()
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !HTTP_SUCCESS.contains(&status) {
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

/// Ask the gateway that holds the credentials to refresh one provider's
/// grants, and return its verdict.
///
/// A refresh run in an operator shell refreshes that shell's own view. The
/// block that empties a pool lives in the gateway's journal, so a local
/// refresh cannot clear it: on 2026-09-21 the pool on charless-mac-mini held
/// one live codex credential while every request answered
/// `503 subscription_reauthorization_required` with `attempts: 0` — every
/// candidate skipped unasked inside a recorded block — and the only refresh
/// the CLI could run was the laptop's. The gateway has exposed
/// `POST /v1/admin/subscription-pool/refresh` all along; this is the verb
/// that reaches it.
pub(crate) async fn refresh(
    gateway: &str,
    bearer: &str,
    provider: &str,
    reason: &str,
) -> Result<Value, String> {
    let provider = provider.trim();
    if provider.is_empty() {
        return Err("name the provider whose grants should be refreshed".into());
    }
    if reason.trim().is_empty() {
        return Err("--reason must say why this refresh is being run".into());
    }
    let response = client()?
        .post(format!(
            "{}/v1/admin/subscription-pool/refresh",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({"provider": provider, "reason": reason}))
        .send()
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !HTTP_SUCCESS.contains(&status) {
        return Err(format!(
            "the gateway refused to refresh {provider}: HTTP {status}: {}",
            refusal(&body)
        ));
    }
    Ok(body)
}

/// What a refused answer says, wherever the gateway put it.
fn refusal(body: &Value) -> String {
    body.pointer("/error/message")
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .to_owned()
}
