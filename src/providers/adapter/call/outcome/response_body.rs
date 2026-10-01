//! A provider answer read whole, as the UTF-8 text it must be.

use std::collections::HashMap;

use super::super::super::plan::headers::plan_headers;
use super::refusal::transport_refusal;
use crate::types::{ProviderRefusal, Refusal};

pub(in crate::providers::adapter) async fn response_text(
    mut response: reqwest::Response,
) -> Result<(reqwest::StatusCode, HashMap<String, String>, String), Refusal> {
    let status = response.status();
    let plan = plan_headers(response.headers());
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| transport_refusal(&error))?
    {
        body.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(body).map_err(|_| {
        Refusal::new(
            ProviderRefusal::ProviderFailure,
            "provider_failure: provider response is not UTF-8",
        )
    })?;
    Ok((status, plan, text))
}
