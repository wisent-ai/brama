//! One admitted exchange with Weles for an account Brama names: buying a new
//! account (`/subscriptions/acquire`) or completing one OAuth authorization
//! a coding-agent harness started for an account the pool holds
//! (`/reauth/authorize`).
//!
//! Both are admitted by the same narrow bearer Brama's sign-ins use, because
//! each spends a real browser run on exactly the account Brama asked for. The
//! answer streams one JSON event per line exactly as a sign-in does, so the
//! CLI, the gateway's log and Desktop say what the run is doing while it
//! runs; the last event is `result`.

use serde_json::{json, Value};

use crate::subscription_dispatch::sign_in::worker::api::{worker_api_base, worker_api_token};
use crate::subscription_dispatch::sign_in::worker::progress::{self, Progress};

/// What one exchange came back with.
pub(super) struct Exchange {
    /// Weles's final answer; `None` when the exchange ended without one.
    pub answer: Option<Value>,
    /// The run's id, host, stages and any operator request it waited on.
    pub observed: Value,
    /// HTTP status of the admission, when Weles answered at all.
    pub http_status: Option<u16>,
    /// Why the exchange ended without an answer, when it did.
    pub transport_failure: Option<String>,
}

/// The Weles provider name for a Brama provider, for the providers Weles
/// can buy an account of and authorize a harness for.
pub(super) fn weles_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "claude-code" => Some("claude"),
        _ => None,
    }
}

/// Ask Weles to buy one account of `provider` on `plan_tier`, stored as
/// `subscription_id`.
pub(super) async fn purchase(
    provider: &str,
    subscription_id: &str,
    plan_tier: &str,
    reason: &str,
    progress_sink: Option<&Progress>,
) -> Result<Exchange, String> {
    let weles_provider = weles_provider(provider)
        .ok_or_else(|| format!("Weles buys claude-code accounts; `{provider}` is not one of them"))?;
    exchange(
        "/subscriptions/acquire",
        json!({
            "provider": weles_provider,
            "subscription_id": subscription_id,
            "plan_tier": plan_tier,
            "reason": reason,
        }),
        progress_sink,
    )
    .await
}

/// Ask Weles to complete the OAuth authorization at `authorize_url` as the
/// account of `subscription_id`, and answer the redirect the provider sent.
pub(super) async fn authorize(
    provider: &str,
    subscription_id: &str,
    authorize_url: &str,
    progress_sink: Option<&Progress>,
) -> Result<Exchange, String> {
    let weles_provider = weles_provider(provider).ok_or_else(|| {
        format!("Weles authorizes claude-code harnesses; `{provider}` is not one of them")
    })?;
    exchange(
        "/reauth/authorize",
        json!({
            "provider": weles_provider,
            "subscription_id": subscription_id,
            "authorize_url": authorize_url,
        }),
        progress_sink,
    )
    .await
}

async fn exchange(
    path: &str,
    body: Value,
    progress_sink: Option<&Progress>,
) -> Result<Exchange, String> {
    let endpoint = worker_api_base().await?;
    let token = worker_api_token()?;
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("Weles HTTP client: {error}"))?;
    let base = &endpoint.url;
    let response = match client
        .post(format!("{base}{path}"))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(Exchange {
                answer: None,
                observed: Value::Null,
                http_status: None,
                transport_failure: Some(format!(
                    "the result of POST {base}{path} is unconfirmed: {error:?}. {}",
                    endpoint.whereabouts()
                )),
            })
        }
    };
    let status = response.status().as_u16();
    let streamed = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with(progress::PROGRESS_CONTENT_TYPE));
    if !streamed {
        let answer = response.json::<Value>().await.map_err(|error| {
            format!("Weles HTTP {status} answered POST {path} with an unreadable body: {error}")
        })?;
        return Ok(Exchange {
            answer: Some(answer),
            observed: Value::Null,
            http_status: Some(status),
            transport_failure: None,
        });
    }
    let mut observed = json!({});
    match progress::read(response, progress_sink).await {
        Ok(read) => {
            read.record(&mut observed);
            let transport_failure = match read.result {
                Some(_) => None,
                None => Some(format!(
                    "Weles ended the answer to POST {path} without a result; {}",
                    read.whereabouts()
                )),
            };
            Ok(Exchange {
                answer: read.result.clone(),
                observed,
                http_status: Some(status),
                transport_failure,
            })
        }
        Err(stopped) => {
            let (read, detail) = *stopped;
            read.record(&mut observed);
            Ok(Exchange {
                answer: None,
                observed,
                http_status: Some(status),
                transport_failure: Some(format!(
                    "{detail}; {}. {}",
                    read.whereabouts(),
                    endpoint.whereabouts()
                )),
            })
        }
    }
}
