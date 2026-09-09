//! How many bytes of a provider answer may be read before the call is called
//! excessive.

use std::collections::HashMap;

use super::super::plan::headers::plan_headers;
use super::refusal::transport_error_message;

fn max_provider_response_bytes() -> usize {
    "16777216"
        .parse()
        .expect("valid provider response byte limit")
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
