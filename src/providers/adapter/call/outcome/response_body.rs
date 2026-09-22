//! How many bytes of a provider answer may be read before the call is called
//! excessive.

use std::collections::HashMap;

use super::super::super::plan::headers::plan_headers;
use super::refusal::transport_error_message;

/// A provider answer is read up to 16 MiB.
const MAX_PROVIDER_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

fn max_provider_response_bytes() -> usize {
    MAX_PROVIDER_RESPONSE_BYTES
}

pub(in crate::providers::adapter) async fn bounded_response_text(
    mut response: reqwest::Response,
) -> Result<(reqwest::StatusCode, HashMap<String, String>, String), String> {
    let status = response.status();
    let plan = plan_headers(response.headers());
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| transport_error_message(&error))?
    {
        if body.len().saturating_add(chunk.len()) > max_provider_response_bytes() {
            return Err("provider_failure: provider response exceeded byte limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(body)
        .map_err(|_| "provider_failure: provider response is not UTF-8".to_string())?;
    Ok((status, plan, text))
}
