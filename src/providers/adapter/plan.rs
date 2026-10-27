//! One bounded, free question about the plan behind a credential, and every
//! refusal it can produce, each carrying the coordinates of the repair.

pub(in crate::providers::adapter) mod endpoint;
pub(in crate::providers::adapter) mod headers;
pub(in crate::providers::adapter) mod probe;
mod report;
pub mod resets;
mod window;

use serde_json::Value;
use tracing::warn;

use super::call::control_client;
use super::call::credential::{authorize_provider, credential_key};
use super::call::outcome::refusal::{provider_refusal, refusal_class, transport_refusal};
use super::call::outcome::response_body::response_text;
use super::registry::{provider, provider_base_url, ProviderDescriptor};
use crate::types::{LimitReading, ProviderRefusal};
use endpoint::{plan_tier_endpoint, plan_usage_endpoint, PlanUsageEndpoint};
use headers::observed_at_ms;
use report::plan_usage_readings;

/// What one provider's own usage report said.
#[derive(Debug)]
pub enum PlanUsage {
    /// The provider answered with a schema-valid report. No readings is valid
    /// only when the provider explicitly reported all of its optional windows
    /// as absent.
    Report(Vec<LimitReading>),
    /// Brama supports no free usage-report endpoint for this provider
    /// credential. Separately privileged billing APIs are outside this flow.
    Unpublished,
    /// The provider call or its response was refused: the class it reads as,
    /// the HTTP status when a response existed, and a detail that names the
    /// endpoint, operation, vault item and underlying cause.
    Refused {
        class: ProviderRefusal,
        status: Option<reqwest::StatusCode>,
        detail: String,
    },
}

/// Whether Brama supports a free usage report for this provider credential.
pub fn publishes_plan_usage(provider_id: &str) -> bool {
    plan_usage_endpoint(provider_id).is_some()
}

/// The usage route to call, with this deployment's own override respected.
///
/// The declared entry carries the path; the origin comes from the same
/// validated, override-aware base the chat route uses, so a host that points a
/// provider at a proxy does not keep one of that provider's two endpoints
/// pointed at the open internet, and the trusted-host policy is enforced on
/// both. Joining an absolute path replaces the base path, which is what makes
/// Codex work at all: its chat route and its usage route are siblings, not
/// parent and child.
fn plan_usage_url(
    descriptor: &ProviderDescriptor,
    endpoint: &PlanUsageEndpoint,
) -> Result<String, String> {
    let declared = reqwest::Url::parse(endpoint.url).map_err(|error| {
        format!(
            "provider `{}` has an invalid usage report URL: {error}",
            descriptor.id
        )
    })?;
    let base = reqwest::Url::parse(&provider_base_url(descriptor)?).map_err(|error| {
        format!(
            "provider `{}` has an invalid base URL: {error}",
            descriptor.id
        )
    })?;
    base.join(declared.path())
        .map(|url| url.to_string())
        .map_err(|error| {
            format!(
                "provider `{}` usage report URL cannot be resolved: {error}",
                descriptor.id
            )
        })
}

fn plan_usage_refusal(
    class: ProviderRefusal,
    provider_id: &str,
    item: &str,
    url: &str,
    status: Option<reqwest::StatusCode>,
    cause: &str,
) -> PlanUsage {
    let answered = match status {
        Some(status) => format!(" returned HTTP {}", status.as_u16()),
        None => " failed before an HTTP response".to_string(),
    };
    let detail = format!(
        "{}: provider `{provider_id}` usage GET `{url}` for credential `{item}`{answered}: {cause}",
        class.contract_kind()
    );
    PlanUsage::Refused {
        class,
        status,
        detail,
    }
}

/// Read one subscription's plan windows from the provider's own usage report.
///
/// This performs one bounded, provider-only GET. It never invokes inference or
/// sign-in, and every refusal retains the operation coordinates needed to
/// diagnose the exact account and endpoint.
pub async fn read_plan_usage(provider_id: &str, item: &str, secret: &str) -> PlanUsage {
    let Some(endpoint) = plan_usage_endpoint(provider_id) else {
        return PlanUsage::Unpublished;
    };
    let failure = ProviderRefusal::ProviderFailure;
    let Some(descriptor) = provider(provider_id) else {
        let cause = "the provider publishes a usage report but is not registered";
        return plan_usage_refusal(failure, provider_id, item, endpoint.url, None, cause);
    };
    let url = match plan_usage_url(descriptor, endpoint) {
        Ok(url) => url,
        Err(message) => {
            return plan_usage_refusal(failure, provider_id, item, endpoint.url, None, &message);
        }
    };
    let key = match credential_key(item, secret) {
        Ok(key) => key,
        Err(message) => {
            let class = ProviderRefusal::Authentication;
            return plan_usage_refusal(class, provider_id, item, &url, None, &message);
        }
    };
    let client = match control_client() {
        Ok(client) => client,
        Err(message) => {
            let cause = format!("provider client configuration failed: {message}");
            return plan_usage_refusal(failure, provider_id, item, &url, None, &cause);
        }
    };
    let builder = client.get(&url).header("accept", "application/json");
    let builder = match authorize_provider(builder, descriptor, &key, secret) {
        Ok(builder) => builder,
        Err(message) => {
            return plan_usage_refusal(
                ProviderRefusal::Authentication,
                provider_id,
                item,
                &url,
                None,
                &message,
            )
        }
    };
    let sent = stado_wait::until(
        stado_wait::Kind::Network,
        "read provider usage report",
        url.as_str(),
        builder.send(),
    )
    .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            let refused = transport_refusal(&error);
            return plan_usage_refusal(
                refused.class,
                provider_id,
                item,
                &url,
                None,
                &refused.message,
            );
        }
    };
    let response_status = response.status();
    let (status, _plan, text) = match response_text(response).await {
        Ok(parts) => parts,
        Err(refused) => {
            let status = Some(response_status);
            return plan_usage_refusal(
                refused.class,
                provider_id,
                item,
                &url,
                status,
                &refused.message,
            );
        }
    };
    if !status.is_success() {
        let class = refusal_class(status, &text);
        let (kind, detail) = provider_refusal(status, &text);
        warn!(
            event = "plan_usage_refused",
            provider = provider_id,
            status = status.as_u16(),
            contract_kind = kind,
            "the provider refused its own usage report: {detail}"
        );
        return plan_usage_refusal(class, provider_id, item, &url, Some(status), &detail);
    }
    let body = match serde_json::from_str::<Value>(&text) {
        Ok(body) => body,
        Err(error) => {
            let cause = format!("usage response is not valid JSON: {error}");
            return plan_usage_refusal(failure, provider_id, item, &url, Some(status), &cause);
        }
    };
    match plan_usage_readings(endpoint.shape, &body, observed_at_ms()) {
        Ok(readings) => PlanUsage::Report(readings),
        Err(error) => {
            let cause = format!("usage response schema is invalid: {error}");
            plan_usage_refusal(failure, provider_id, item, &url, Some(status), &cause)
        }
    }
}

/// The plan tier the provider says this credential's account holds, read
/// from its own profile with one bounded GET that spends no quota.
///
/// `Err` carries the endpoint, the vault item, the HTTP status when one came
/// back and the provider's own words, so a refusal names what to repair. A
/// provider that states no plan tier is refused by name rather than answered
/// with a guess.
pub async fn read_plan_tier(provider_id: &str, item: &str, secret: &str) -> Result<String, String> {
    let endpoint = plan_tier_endpoint(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` states no plan tier Brama can read"))?;
    let descriptor = provider(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` is not registered"))?;
    let url = plan_usage_url(descriptor, endpoint)?;
    let key = credential_key(item, secret)
        .map_err(|message| format!("profile GET `{url}` for credential `{item}`: {message}"))?;
    let client = control_client()
        .map_err(|message| format!("provider client configuration failed: {message}"))?;
    let builder = client.get(&url).header("accept", "application/json");
    let builder = authorize_provider(builder, descriptor, &key, secret)
        .map_err(|message| format!("profile GET `{url}` for credential `{item}`: {message}"))?;
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        "read provider plan tier",
        url.as_str(),
        builder.send(),
    )
    .await
    .map_err(|error| {
        format!(
            "profile GET `{url}` for credential `{item}` failed before an HTTP response: {}",
            transport_refusal(&error).message
        )
    })?;
    let (status, _plan, text) = response_text(response).await.map_err(|refused| {
        format!(
            "profile GET `{url}` for credential `{item}`: {}",
            refused.message
        )
    })?;
    if !status.is_success() {
        let (kind, detail) = provider_refusal(status, &text);
        return Err(format!(
            "{kind}: profile GET `{url}` for credential `{item}` returned HTTP {}: {detail}",
            status.as_u16()
        ));
    }
    let body = serde_json::from_str::<Value>(&text).map_err(|error| {
        format!("profile GET `{url}` for credential `{item}` is not valid JSON: {error}")
    })?;
    body.pointer("/organization/rate_limit_tier")
        .and_then(Value::as_str)
        .filter(|tier| !tier.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "profile GET `{url}` for credential `{item}` answered without \
                 organization.rate_limit_tier: {body}"
            )
        })
}
