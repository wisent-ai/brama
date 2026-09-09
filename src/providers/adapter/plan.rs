//! One bounded, free question about the plan behind a credential, and every
//! refusal it can produce, each carrying the coordinates of the repair.

pub(in crate::providers::adapter) mod endpoint;
pub(in crate::providers::adapter) mod headers;
pub(in crate::providers::adapter) mod probe;
mod report;
mod window;

use serde_json::Value;
use tracing::warn;

use super::call::control_client;
use super::call::credential::{authorize_provider, credential_key};
use super::call::refusal::{provider_refusal, transport_error_message};
use super::call::response_body::bounded_response_text;
use super::registry::{provider, provider_base_url, ProviderDescriptor};
use crate::types::LimitReading;
use endpoint::{plan_usage_endpoint, PlanUsageEndpoint};
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
    /// The provider call or its response was refused. The string retains the
    /// contract kind prefix and the exact endpoint, operation, status (when an
    /// HTTP response existed), vault item, and underlying cause.
    Refused(String),
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
    kind: &str,
    provider_id: &str,
    item: &str,
    url: &str,
    status: Option<reqwest::StatusCode>,
    cause: &str,
) -> PlanUsage {
    let status = status
        .map(|status| format!(" returned HTTP {}", status.as_u16()))
        .unwrap_or_else(|| " failed before an HTTP response".to_string());
    PlanUsage::Refused(format!(
        "{kind}: provider `{provider_id}` usage GET `{url}` for credential `{item}`{status}: \
         {cause}"
    ))
}

fn classified_plan_usage_refusal(
    provider_id: &str,
    item: &str,
    url: &str,
    status: Option<reqwest::StatusCode>,
    message: &str,
) -> PlanUsage {
    let (kind, cause) = message
        .split_once(':')
        .map(|(kind, cause)| (kind.trim(), cause.trim()))
        .unwrap_or(("provider_failure", message));
    plan_usage_refusal(kind, provider_id, item, url, status, cause)
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
    let Some(descriptor) = provider(provider_id) else {
        return plan_usage_refusal(
            "provider_failure",
            provider_id,
            item,
            endpoint.url,
            None,
            "the provider publishes a usage report but is not registered",
        );
    };
    let url = match plan_usage_url(descriptor, endpoint) {
        Ok(url) => url,
        Err(message) => {
            return plan_usage_refusal(
                "provider_failure",
                provider_id,
                item,
                endpoint.url,
                None,
                &message,
            );
        }
    };
    let key = match credential_key(item, secret) {
        Ok(key) => key,
        Err(message) => {
            return plan_usage_refusal(
                "provider_authentication",
                provider_id,
                item,
                &url,
                None,
                &message,
            );
        }
    };
    let client = match control_client() {
        Ok(client) => client,
        Err(message) => {
            return plan_usage_refusal(
                "provider_failure",
                provider_id,
                item,
                &url,
                None,
                &format!("provider client configuration failed: {message}"),
            );
        }
    };
    let builder = client.get(&url).header("accept", "application/json");
    let response = match authorize_provider(builder, descriptor, &key, secret)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return classified_plan_usage_refusal(
                provider_id,
                item,
                &url,
                None,
                &transport_error_message(&error),
            );
        }
    };
    let response_status = response.status();
    let (status, _plan, text) = match bounded_response_text(response).await {
        Ok(parts) => parts,
        Err(message) => {
            return classified_plan_usage_refusal(
                provider_id,
                item,
                &url,
                Some(response_status),
                &message,
            );
        }
    };
    if !status.is_success() {
        let (kind, detail) = provider_refusal(status, &text);
        warn!(
            event = "plan_usage_refused",
            provider = provider_id,
            status = status.as_u16(),
            contract_kind = kind,
            "the provider refused its own usage report: {detail}"
        );
        return plan_usage_refusal(kind, provider_id, item, &url, Some(status), &detail);
    }
    let body = match serde_json::from_str::<Value>(&text) {
        Ok(body) => body,
        Err(error) => {
            return plan_usage_refusal(
                "provider_failure",
                provider_id,
                item,
                &url,
                Some(status),
                &format!("usage response is not valid JSON: {error}"),
            );
        }
    };
    match plan_usage_readings(endpoint.shape, &body, observed_at_ms()) {
        Ok(readings) => PlanUsage::Report(readings),
        Err(error) => plan_usage_refusal(
            "provider_failure",
            provider_id,
            item,
            &url,
            Some(status),
            &format!("usage response schema is invalid: {error}"),
        ),
    }
}
